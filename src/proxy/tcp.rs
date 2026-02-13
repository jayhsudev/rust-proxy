use log::info;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::task;

use crate::common::auth::AuthManager;
use crate::net::conn::BufferedConnection;
use crate::proxy::http::HttpProxy;
use crate::proxy::socks5::Socks5Proxy;

/// TCP代理错误
#[derive(Error, Debug)]
pub enum TcpProxyError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("No data received from client")]
    NoDataReceived,
    #[error("Unsupported protocol")]
    UnsupportedProtocol,
    #[error("Proxy error: {0}")]
    ProxyError(Box<dyn std::error::Error>),
}

/// TCP代理
pub struct TcpProxy {
    /// 身份验证管理器
    auth_manager: Arc<AuthManager>,
}

impl TcpProxy {
    /// 创建新的TCP代理
    pub fn new(auth_manager: Arc<AuthManager>) -> Self {
        TcpProxy { auth_manager }
    }

    /// 运行TCP代理
    pub async fn run(&self, listener: TcpListener) {
        info!("TCP proxy listening on {}", listener.local_addr().unwrap());

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    let auth_manager = self.auth_manager.clone();
                    task::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, addr, auth_manager).await {
                            log::error!("Error handling connection from {}: {}", addr, e);
                        }
                    });
                }
                Err(e) => {
                    log::error!("Error accepting connection: {}", e);
                    // 短暂睡眠后重试
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// 处理单个TCP连接
    async fn handle_connection(
        stream: TcpStream,
        addr: std::net::SocketAddr,
        auth_manager: Arc<AuthManager>,
    ) -> Result<(), TcpProxyError> {
        // 设置TCP_NODELAY选项
        stream.set_nodelay(true)?;

        // 创建带缓冲的连接（使用默认缓冲区大小）
        // 注意：在实际应用中，应该从配置中获取缓冲区大小
        let mut conn = BufferedConnection::new(stream, 4096);

        // 读取一些数据来确定协议类型
        let bytes_read = conn.read().await?;

        if bytes_read == 0 || !conn.has_data() {
            return Err(TcpProxyError::NoDataReceived);
        }

        // 检查协议类型
        let first_byte = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0],
            None => return Err(TcpProxyError::NoDataReceived),
        };
        conn.unread(&[first_byte]); // 放回缓冲区

        // 根据第一个字节判断协议类型
        match first_byte {
            // SOCKS5协议以0x05开头
            0x05 => {
                info!("SOCKS5 connection from {}", addr);
                let mut socks5_proxy = Socks5Proxy::new(auth_manager);
                socks5_proxy
                    .handle_connection(&mut conn)
                    .await
                    .map_err(|e| TcpProxyError::ProxyError(Box::new(e)))?;
            }
            // HTTP协议通常以字母开头 (GET, POST, etc.)
            b'A'..=b'Z' => {
                info!("HTTP connection from {}", addr);
                let mut http_proxy = HttpProxy::new(auth_manager);
                http_proxy
                    .handle_connection(&mut conn)
                    .await
                    .map_err(TcpProxyError::ProxyError)?;
            }
            // 可能是HTTP CONNECT方法或其他协议
            _ => {
                // 尝试读取更多数据以确定协议类型
                if conn.available_bytes() < 3 {
                    conn.read().await?;
                }

                // 检查是否是HTTP CONNECT (通常以 "CONNECT" 开头)
                if conn.available_bytes() >= 7 {
                    let peek_data = match conn.read_from_buffer(7) {
                        Some(data) => data,
                        None => return Err(TcpProxyError::UnsupportedProtocol),
                    };
                    conn.unread(&peek_data); // 放回缓冲区

                    if peek_data.eq_ignore_ascii_case(b"CONNECT") {
                        info!("HTTP CONNECT connection from {}", addr);
                        let mut http_proxy = HttpProxy::new(auth_manager);
                        http_proxy
                            .handle_connection(&mut conn)
                            .await
                            .map_err(TcpProxyError::ProxyError)?;
                        return Ok(());
                    }
                }

                // 如果仍然无法确定协议类型，则返回错误
                return Err(TcpProxyError::UnsupportedProtocol);
            }
        }

        Ok(())
    }
}
