use log::{debug, warn};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use thiserror::Error;
use tokio::time::timeout;

use crate::common::auth::{AuthError, AuthManager};
use crate::common::metrics::Metrics;
use crate::net::conn::BufferedConnection;
use crate::proxy::forward::{self, Upstream};

#[derive(Error, Debug)]
pub enum Socks5ProxyError {
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("Invalid SOCKS version: {0:#04x}")]
    InvalidVersion(u8),
    #[error("No supported authentication method")]
    NoSupportedAuthMethod,
    #[error("Invalid authentication sub-negotiation version: {0:#04x}")]
    InvalidAuthVersion(u8),
    #[error("Authentication failed")]
    AuthenticationFailed(#[from] AuthError),
    #[error("Unsupported command: {0:#04x}")]
    UnsupportedCommand(u8),
    #[error("Invalid address type: {0:#04x}")]
    InvalidAddressType(u8),
    #[error("Invalid reserved byte: {0:#04x}")]
    InvalidReservedByte(u8),
    #[error("Client handshake timed out")]
    HandshakeTimeout,
    #[error("Connection error: {0}")]
    ConnectError(#[from] forward::ConnectError),
    #[error("Invalid UTF-8 data")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

const SOCKS_VERSION: u8 = 0x05;
const METHOD_NO_AUTH: u8 = 0x00;
const METHOD_USERNAME_PASSWORD: u8 = 0x02;
const METHOD_NOT_ACCEPTABLE: u8 = 0xFF;

// SOCKS5 reply codes (RFC 1928 §6)
const REPLY_SUCCEEDED: u8 = 0x00;
const REPLY_GENERAL_FAILURE: u8 = 0x01;
const REPLY_HOST_UNREACHABLE: u8 = 0x04;
const REPLY_CONNECTION_REFUSED: u8 = 0x05;
const REPLY_TTL_EXPIRED: u8 = 0x06;
const REPLY_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const REPLY_ADDRESS_TYPE_NOT_SUPPORTED: u8 = 0x08;

pub struct Socks5Proxy {
    auth_manager: Arc<AuthManager>,
    upstream: Arc<Upstream>,
    metrics: Arc<Metrics>,
}

impl Socks5Proxy {
    pub fn new(
        auth_manager: Arc<AuthManager>,
        upstream: Arc<Upstream>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Socks5Proxy {
            auth_manager,
            upstream,
            metrics,
        }
    }

    pub async fn handle_connection(
        &self,
        mut conn: BufferedConnection,
    ) -> Result<(), Socks5ProxyError> {
        let handshake_timeout = self.upstream.options().handshake_timeout;

        match timeout(handshake_timeout, self.greet(&mut conn)).await {
            Ok(result) => {
                result?;
            }
            Err(_) => return Err(Socks5ProxyError::HandshakeTimeout),
        }

        let target_addr_str = match timeout(handshake_timeout, self.handle_request(&mut conn)).await
        {
            Ok(Ok(addr)) => addr,
            Ok(Err(e)) => {
                let _ = self.send_reply(&mut conn, reply_code_for(&e), None).await;
                return Err(e);
            }
            Err(_) => {
                let _ = self
                    .send_reply(&mut conn, REPLY_GENERAL_FAILURE, None)
                    .await;
                return Err(Socks5ProxyError::HandshakeTimeout);
            }
        };

        let target_stream = match self.upstream.connect(&target_addr_str).await {
            Ok(stream) => stream,
            Err(e) => {
                self.metrics.upstream_error();
                let _ = self
                    .send_reply(&mut conn, reply_code_for_connect(&e), None)
                    .await;
                return Err(Socks5ProxyError::ConnectError(e));
            }
        };

        let bound_addr = target_stream.local_addr().ok();
        self.send_reply(&mut conn, REPLY_SUCCEEDED, bound_addr)
            .await?;
        self.metrics.socks5_connect();
        debug!("SOCKS5 CONNECT {} established", target_addr_str);

        let buffer_size = conn.buffer_size();
        let target_conn = BufferedConnection::new(target_stream, buffer_size);

        match forward::relay(conn, target_conn, self.upstream.options().relay_buffer_size).await {
            Ok((up, down)) => {
                self.metrics.add_bytes(up, down);
                Ok(())
            }
            Err(e) => {
                self.metrics.upstream_error();
                Err(Socks5ProxyError::IoError(e))
            }
        }
    }

    /// Method negotiation followed by the optional RFC 1929 sub-negotiation.
    async fn greet(&self, conn: &mut BufferedConnection) -> Result<(), Socks5ProxyError> {
        let selected_method = self.handshake(conn).await?;

        if selected_method == METHOD_USERNAME_PASSWORD {
            self.authenticate(conn).await?;
        }

        Ok(())
    }

    async fn handshake(&self, conn: &mut BufferedConnection) -> Result<u8, Socks5ProxyError> {
        let header = conn.read_exact_bytes(2).await?;
        let version = header[0];
        let nmethods = header[1] as usize;

        if version != SOCKS_VERSION {
            return Err(Socks5ProxyError::InvalidVersion(version));
        }

        if nmethods == 0 {
            conn.write(&[SOCKS_VERSION, METHOD_NOT_ACCEPTABLE]).await?;
            return Err(Socks5ProxyError::NoSupportedAuthMethod);
        }

        let methods = conn.read_exact_bytes(nmethods).await?;

        let selected_method = if self.auth_manager.has_users() {
            if methods.contains(&METHOD_USERNAME_PASSWORD) {
                METHOD_USERNAME_PASSWORD
            } else {
                conn.write(&[SOCKS_VERSION, METHOD_NOT_ACCEPTABLE]).await?;
                return Err(Socks5ProxyError::NoSupportedAuthMethod);
            }
        } else if methods.contains(&METHOD_NO_AUTH) {
            METHOD_NO_AUTH
        } else if methods.contains(&METHOD_USERNAME_PASSWORD) {
            // No credentials configured: accept the sub-negotiation and let it pass.
            METHOD_USERNAME_PASSWORD
        } else {
            conn.write(&[SOCKS_VERSION, METHOD_NOT_ACCEPTABLE]).await?;
            return Err(Socks5ProxyError::NoSupportedAuthMethod);
        };

        conn.write(&[SOCKS_VERSION, selected_method]).await?;

        Ok(selected_method)
    }

    /// RFC 1929 Username/Password sub-negotiation:
    /// +----+------+----------+------+----------+
    /// |VER | ULEN |  UNAME   | PLEN |  PASSWD  |
    /// +----+------+----------+------+----------+
    /// | 1  |  1   | 1 to 255 |  1   | 1 to 255 |
    /// +----+------+----------+------+----------+
    async fn authenticate(&self, conn: &mut BufferedConnection) -> Result<(), Socks5ProxyError> {
        let header = conn.read_exact_bytes(2).await?;
        let auth_version = header[0];
        let username_len = header[1] as usize;

        if auth_version != 0x01 {
            conn.write(&[0x01, 0x01]).await?;
            return Err(Socks5ProxyError::InvalidAuthVersion(auth_version));
        }

        let username = String::from_utf8(conn.read_exact_bytes(username_len).await?)?;
        let password_len = conn.read_exact_bytes(1).await?[0] as usize;
        let password = String::from_utf8(conn.read_exact_bytes(password_len).await?)?;

        let auth_success = match self.auth_manager.authenticate(&username, &password).await {
            Ok(result) => result,
            Err(e) => {
                conn.write(&[0x01, 0x01]).await?;
                return Err(Socks5ProxyError::AuthenticationFailed(e));
            }
        };

        let status = if auth_success { 0x00 } else { 0x01 };
        conn.write(&[0x01, status]).await?;

        if !auth_success {
            self.metrics.auth_failure();
            warn!("SOCKS5 authentication failed for user '{}'", username);
            return Err(Socks5ProxyError::AuthenticationFailed(
                AuthError::AuthenticationFailed,
            ));
        }

        debug!("SOCKS5 user '{}' authenticated", username);
        Ok(())
    }

    async fn handle_request(
        &self,
        conn: &mut BufferedConnection,
    ) -> Result<String, Socks5ProxyError> {
        let header = conn.read_exact_bytes(4).await?;
        let version = header[0];
        let command = header[1];
        let reserved = header[2];
        let addr_type = header[3];

        if version != SOCKS_VERSION {
            return Err(Socks5ProxyError::InvalidVersion(version));
        }

        if reserved != 0x00 {
            return Err(Socks5ProxyError::InvalidReservedByte(reserved));
        }

        if command != 0x01 {
            return Err(Socks5ProxyError::UnsupportedCommand(command));
        }

        let addr_str = match addr_type {
            // IPv4
            0x01 => {
                let data = conn.read_exact_bytes(4).await?;
                let port = read_port(conn).await?;
                let ip = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
                format!("{}:{}", ip, port)
            }
            // Domain name
            0x03 => {
                let domain_len = conn.read_exact_bytes(1).await?[0] as usize;
                if domain_len == 0 {
                    return Err(Socks5ProxyError::InvalidAddressType(addr_type));
                }
                let domain = String::from_utf8(conn.read_exact_bytes(domain_len).await?)?;
                let port = read_port(conn).await?;
                forward::join_host_port(&domain, port)
            }
            // IPv6
            0x04 => {
                let data = conn.read_exact_bytes(16).await?;
                let port = read_port(conn).await?;
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&data);
                let ip = Ipv6Addr::from(octets);
                format!("[{}]:{}", ip, port)
            }
            _ => return Err(Socks5ProxyError::InvalidAddressType(addr_type)),
        };

        Ok(addr_str)
    }

    async fn send_reply(
        &self,
        conn: &mut BufferedConnection,
        reply_code: u8,
        bound_addr: Option<SocketAddr>,
    ) -> Result<(), Socks5ProxyError> {
        let mut reply = Vec::with_capacity(22);
        reply.push(SOCKS_VERSION);
        reply.push(reply_code);
        reply.push(0x00); // RSV

        match bound_addr {
            Some(SocketAddr::V4(addr)) => {
                reply.push(0x01);
                reply.extend_from_slice(&addr.ip().octets());
                reply.extend_from_slice(&addr.port().to_be_bytes());
            }
            Some(SocketAddr::V6(addr)) => {
                reply.push(0x04);
                reply.extend_from_slice(&addr.ip().octets());
                reply.extend_from_slice(&addr.port().to_be_bytes());
            }
            None => {
                reply.push(0x01);
                reply.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
            }
        }

        conn.write(&reply).await?;
        Ok(())
    }
}

async fn read_port(conn: &mut BufferedConnection) -> Result<u16, Socks5ProxyError> {
    let port_bytes = conn.read_exact_bytes(2).await?;
    Ok(u16::from_be_bytes([port_bytes[0], port_bytes[1]]))
}

fn reply_code_for(error: &Socks5ProxyError) -> u8 {
    match error {
        Socks5ProxyError::UnsupportedCommand(_) => REPLY_COMMAND_NOT_SUPPORTED,
        Socks5ProxyError::InvalidAddressType(_) => REPLY_ADDRESS_TYPE_NOT_SUPPORTED,
        _ => REPLY_GENERAL_FAILURE,
    }
}

fn reply_code_for_connect(error: &forward::ConnectError) -> u8 {
    match error {
        forward::ConnectError::ConnectionRefused(_) => REPLY_CONNECTION_REFUSED,
        forward::ConnectError::ConnectionTimeout => REPLY_TTL_EXPIRED,
        forward::ConnectError::AddressResolutionFailed(_)
        | forward::ConnectError::AddressNotFound => REPLY_HOST_UNREACHABLE,
        forward::ConnectError::IoError(_) => REPLY_GENERAL_FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::options::ProxyOptions;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Starts an echo server and returns its address.
    async fn start_echo_server() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let (mut reader, mut writer) = stream.split();
                    let _ = tokio::io::copy(&mut reader, &mut writer).await;
                });
            }
        });
        addr
    }

    /// Starts the SOCKS5 handler on an ephemeral port, returns its address.
    async fn start_proxy(require_auth: bool) -> std::net::SocketAddr {
        let mut users = HashMap::new();
        if require_auth {
            users.insert("alice".to_string(), "secret".to_string());
        }
        let auth_manager = Arc::new(AuthManager::new(&users).unwrap());
        let upstream = Arc::new(Upstream::new(ProxyOptions::for_tests()));
        let metrics = Metrics::new();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = BufferedConnection::new(stream, 4096);
            let proxy = Socks5Proxy::new(auth_manager, upstream, metrics);
            let _ = proxy.handle_connection(conn).await;
        });

        addr
    }

    #[tokio::test]
    async fn test_connect_to_echo_server() {
        let echo_addr = start_echo_server().await;
        let proxy_addr = start_proxy(false).await;

        let mut client = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();

        // Greeting: version 5, one method: no auth
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut response = [0u8; 2];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(response, [0x05, 0x00]);

        // CONNECT to IPv4 target
        let mut request = vec![0x05, 0x01, 0x00, 0x01];
        request.extend_from_slice(
            &echo_addr
                .ip()
                .to_string()
                .parse::<Ipv4Addr>()
                .unwrap()
                .octets(),
        );
        request.extend_from_slice(&echo_addr.port().to_be_bytes());
        client.write_all(&request).await.unwrap();

        let mut reply = [0u8; 10];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[0], 0x05);
        assert_eq!(reply[1], REPLY_SUCCEEDED);
        assert_eq!(reply[3], 0x01);

        // The bound address must be a real local port.
        let bound_port = u16::from_be_bytes([reply[8], reply[9]]);
        assert_ne!(bound_port, 0);

        client.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        client.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"ping");
    }

    #[tokio::test]
    async fn test_domain_connect_requires_auth() {
        let proxy_addr = start_proxy(true).await;
        let mut client = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();

        // Client only supports "no auth" -> server must reject.
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut response = [0u8; 2];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(response, [0x05, METHOD_NOT_ACCEPTABLE]);
    }

    #[tokio::test]
    async fn test_username_password_auth_success_and_failure() {
        for (password, expected_status) in [("secret", 0x00u8), ("wrong", 0x01u8)] {
            let proxy_addr = start_proxy(true).await;
            let mut client = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();

            client.write_all(&[0x05, 0x01, 0x02]).await.unwrap();
            let mut response = [0u8; 2];
            client.read_exact(&mut response).await.unwrap();
            assert_eq!(response, [0x05, METHOD_USERNAME_PASSWORD]);

            let mut auth = vec![0x01, 5];
            auth.extend_from_slice(b"alice");
            auth.push(password.len() as u8);
            auth.extend_from_slice(password.as_bytes());
            client.write_all(&auth).await.unwrap();

            let mut auth_response = [0u8; 2];
            client.read_exact(&mut auth_response).await.unwrap();
            assert_eq!(auth_response, [0x01, expected_status]);
        }
    }

    #[tokio::test]
    async fn test_unsupported_command_reply() {
        let proxy_addr = start_proxy(false).await;
        let mut client = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();

        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut response = [0u8; 2];
        client.read_exact(&mut response).await.unwrap();

        // BIND is not supported by this proxy.
        client
            .write_all(&[0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0x00, 0x50])
            .await
            .unwrap();

        let mut reply = [0u8; 10];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[1], REPLY_COMMAND_NOT_SUPPORTED);
    }
}
