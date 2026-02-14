use log::info;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

use crate::common::auth::{AuthError, AuthManager};
use crate::net::conn::BufferedConnection;
use crate::proxy::forward;

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
    #[error("Connection error: {0}")]
    ConnectError(#[from] crate::proxy::forward::ConnectError),
    #[error("Invalid UTF-8 data")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

// SOCKS5 reply codes (RFC 1928 §6)
const REPLY_SUCCEEDED: u8 = 0x00;
const REPLY_GENERAL_FAILURE: u8 = 0x01;
const REPLY_HOST_UNREACHABLE: u8 = 0x04;
const REPLY_CONNECTION_REFUSED: u8 = 0x05;
const REPLY_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const REPLY_ADDRESS_TYPE_NOT_SUPPORTED: u8 = 0x08;

pub struct Socks5Proxy {
    auth_manager: Arc<AuthManager>,
    connect_timeout: Duration,
}

impl Socks5Proxy {
    pub fn new(auth_manager: Arc<AuthManager>, connect_timeout: Duration) -> Self {
        Socks5Proxy {
            auth_manager,
            connect_timeout,
        }
    }

    pub async fn handle_connection(
        &self,
        conn: &mut BufferedConnection,
    ) -> Result<(), Socks5ProxyError> {
        info!("Handling SOCKS5 connection");

        let selected_method = self.handshake(conn).await?;

        if selected_method == 0x02 {
            self.authenticate(conn).await?;
        }

        let target_addr_str = match self.handle_request(conn).await {
            Ok(addr) => addr,
            Err(e) => {
                let reply_code = match &e {
                    Socks5ProxyError::UnsupportedCommand(_) => REPLY_COMMAND_NOT_SUPPORTED,
                    Socks5ProxyError::InvalidAddressType(_) => REPLY_ADDRESS_TYPE_NOT_SUPPORTED,
                    _ => REPLY_GENERAL_FAILURE,
                };
                let _ = self.send_reply(conn, reply_code).await;
                return Err(e);
            }
        };

        let target_stream =
            match forward::connect_with_timeout(&target_addr_str, self.connect_timeout).await {
                Ok(stream) => stream,
                Err(e) => {
                    let reply_code = match &e {
                        forward::ConnectError::ConnectionTimeout => REPLY_GENERAL_FAILURE,
                        forward::ConnectError::ConnectionRefused(_) => REPLY_CONNECTION_REFUSED,
                        forward::ConnectError::AddressResolutionFailed(_) => REPLY_HOST_UNREACHABLE,
                        _ => REPLY_GENERAL_FAILURE,
                    };
                    let _ = self.send_reply(conn, reply_code).await;
                    return Err(Socks5ProxyError::ConnectError(e));
                }
            };

        info!("Connected to target: {}", target_addr_str);

        self.send_reply(conn, REPLY_SUCCEEDED).await?;

        let buffer_size = conn.buffer_size();
        let mut target_conn = BufferedConnection::new(target_stream, buffer_size);
        forward::forward_bidirectional(conn, &mut target_conn)
            .await
            .map_err(Socks5ProxyError::IoError)?;

        Ok(())
    }

    async fn handshake(&self, conn: &mut BufferedConnection) -> Result<u8, Socks5ProxyError> {
        let header = conn.read_exact_bytes(2).await?;
        let version = header[0];
        let nmethods = header[1] as usize;

        if version != 0x05 {
            return Err(Socks5ProxyError::InvalidVersion(version));
        }

        let methods = conn.read_exact_bytes(nmethods).await?;

        let selected_method = if self.auth_manager.has_users() {
            if methods.contains(&0x02) {
                info!("Selected username/password authentication");
                0x02
            } else {
                conn.write(&[0x05, 0xFF]).await?;
                return Err(Socks5ProxyError::NoSupportedAuthMethod);
            }
        } else if methods.contains(&0x00) {
            info!("Selected no authentication");
            0x00
        } else if methods.contains(&0x02) {
            info!("Selected username/password authentication (no auth required, client will pass)");
            0x02
        } else {
            conn.write(&[0x05, 0xFF]).await?;
            return Err(Socks5ProxyError::NoSupportedAuthMethod);
        };

        conn.write(&[0x05, selected_method]).await?;
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
            return Err(Socks5ProxyError::AuthenticationFailed(
                AuthError::AuthenticationFailed,
            ));
        }

        info!("User '{}' authenticated", username);
        Ok(())
    }

    async fn handle_request(
        &self,
        conn: &mut BufferedConnection,
    ) -> Result<String, Socks5ProxyError> {
        let header = conn.read_exact_bytes(4).await?;
        let version = header[0];
        let command = header[1];
        let addr_type = header[3];

        if version != 0x05 {
            return Err(Socks5ProxyError::InvalidVersion(version));
        }

        if command != 0x01 {
            return Err(Socks5ProxyError::UnsupportedCommand(command));
        }

        let addr_str = match addr_type {
            // IPv4
            0x01 => {
                let data = conn.read_exact_bytes(4).await?;
                let port_bytes = conn.read_exact_bytes(2).await?;
                let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);
                format!("{}.{}.{}.{}:{}", data[0], data[1], data[2], data[3], port)
            }
            // Domain name
            0x03 => {
                let domain_len = conn.read_exact_bytes(1).await?[0] as usize;
                let domain_bytes = conn.read_exact_bytes(domain_len).await?;
                let domain = String::from_utf8(domain_bytes)?;
                let port_bytes = conn.read_exact_bytes(2).await?;
                let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);
                format!("{}:{}", domain, port)
            }
            // IPv6
            0x04 => {
                let data = conn.read_exact_bytes(16).await?;
                let port_bytes = conn.read_exact_bytes(2).await?;
                let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);
                let ip = std::net::Ipv6Addr::new(
                    u16::from_be_bytes([data[0], data[1]]),
                    u16::from_be_bytes([data[2], data[3]]),
                    u16::from_be_bytes([data[4], data[5]]),
                    u16::from_be_bytes([data[6], data[7]]),
                    u16::from_be_bytes([data[8], data[9]]),
                    u16::from_be_bytes([data[10], data[11]]),
                    u16::from_be_bytes([data[12], data[13]]),
                    u16::from_be_bytes([data[14], data[15]]),
                );
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
    ) -> Result<(), Socks5ProxyError> {
        conn.write(&[
            0x05, reply_code, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ])
        .await?;
        Ok(())
    }
}
