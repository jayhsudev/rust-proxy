use log::info;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task;

use crate::common::auth::AuthManager;
use crate::net::conn::BufferedConnection;
use crate::proxy::http::HttpProxy;
use crate::proxy::socks5::Socks5Proxy;

#[derive(Error, Debug)]
pub enum TcpProxyError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("No data received from client")]
    NoDataReceived,
    #[error("Unsupported protocol (first byte: {0:#04x})")]
    UnsupportedProtocol(u8),
    #[error("HTTP proxy error: {0}")]
    HttpProxyError(#[from] crate::proxy::http::HttpProxyError),
    #[error("SOCKS5 proxy error: {0}")]
    Socks5ProxyError(#[from] crate::proxy::socks5::Socks5ProxyError),
}

pub struct TcpProxy {
    auth_manager: Arc<AuthManager>,
    buffer_size: usize,
    semaphore: Arc<Semaphore>,
    connect_timeout: Duration,
}

impl TcpProxy {
    pub fn new(
        auth_manager: Arc<AuthManager>,
        buffer_size: usize,
        max_connections: usize,
        connect_timeout: Duration,
    ) -> Self {
        TcpProxy {
            auth_manager,
            buffer_size,
            semaphore: Arc::new(Semaphore::new(max_connections)),
            connect_timeout,
        }
    }

    /// Accept connections until Ctrl-C / SIGINT is received.
    pub async fn run(&self, listener: TcpListener) {
        info!("TCP proxy listening on {}", listener.local_addr().unwrap());

        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            let permit = match self.semaphore.clone().try_acquire_owned() {
                                Ok(permit) => permit,
                                Err(_) => {
                                    log::warn!("Max connections reached, rejecting {}", addr);
                                    drop(stream);
                                    continue;
                                }
                            };
                            let auth_manager = self.auth_manager.clone();
                            let buffer_size = self.buffer_size;
                            let connect_timeout = self.connect_timeout;
                            task::spawn(async move {
                                if let Err(e) = Self::handle_connection(
                                    stream,
                                    addr,
                                    auth_manager,
                                    buffer_size,
                                    connect_timeout,
                                )
                                .await
                                {
                                    log::error!("Connection error from {}: {}", addr, e);
                                }
                                drop(permit);
                            });
                        }
                        Err(e) => {
                            log::error!("Accept error: {}", e);
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
                _ = &mut shutdown => {
                    info!("Received shutdown signal");
                    break;
                }
            }
        }

        info!("Stopped accepting new connections");
    }

    async fn handle_connection(
        stream: TcpStream,
        addr: std::net::SocketAddr,
        auth_manager: Arc<AuthManager>,
        buffer_size: usize,
        connect_timeout: Duration,
    ) -> Result<(), TcpProxyError> {
        stream.set_nodelay(true)?;
        let mut conn = BufferedConnection::new(stream, buffer_size);

        let bytes_read = conn.read().await?;
        if bytes_read == 0 || !conn.has_data() {
            return Err(TcpProxyError::NoDataReceived);
        }

        let first_byte = conn
            .read_from_buffer(1)
            .map(|b| b[0])
            .ok_or(TcpProxyError::NoDataReceived)?;
        conn.unread(&[first_byte]);

        match first_byte {
            // SOCKS5 protocol starts with 0x05
            0x05 => {
                info!("SOCKS5 connection from {}", addr);
                let socks5_proxy = Socks5Proxy::new(auth_manager, connect_timeout);
                socks5_proxy.handle_connection(&mut conn).await?;
            }
            // HTTP methods start with ASCII letters
            b'A'..=b'Z' | b'a'..=b'z' => {
                info!("HTTP connection from {}", addr);
                let http_proxy = HttpProxy::new(auth_manager, buffer_size, connect_timeout);
                http_proxy.handle_connection(&mut conn).await?;
            }
            other => {
                return Err(TcpProxyError::UnsupportedProtocol(other));
            }
        }

        Ok(())
    }
}
