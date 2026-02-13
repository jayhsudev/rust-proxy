use log::info;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use thiserror::Error;
use tokio::net::TcpStream;

use crate::common::auth::{AuthError, AuthManager};
use crate::net::conn::BufferedConnection;
use crate::proxy::forward::Forwarder;

/// SOCKS5代理错误
#[derive(Error, Debug)]
pub enum Socks5ProxyError {
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("Invalid SOCKS version")]
    InvalidVersion,
    #[error("No supported authentication method")]
    NoSupportedAuthMethod,
    #[error("Invalid authentication method")]
    InvalidAuthMethod,
    #[error("Authentication failed")]
    AuthenticationFailed(#[from] AuthError),
    #[error("Connection closed during {0}")]
    ConnectionClosed(&'static str),
    #[error("Unsupported command")]
    UnsupportedCommand,
    #[error("Invalid address type")]
    InvalidAddressType,
    #[error("Failed to connect to target: {0}")]
    ConnectTargetFailed(io::Error),
    #[error("Invalid UTF-8 data")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("Failed to resolve address: {0}")]
    AddressResolutionFailed(String),
}

/// SOCKS5代理
pub struct Socks5Proxy {
    /// 身份验证管理器
    auth_manager: Arc<AuthManager>,
}

impl Socks5Proxy {
    /// 创建新的SOCKS5代理
    pub fn new(auth_manager: Arc<AuthManager>) -> Self {
        Socks5Proxy { auth_manager }
    }

    /// 处理SOCKS5连接
    pub async fn handle_connection(
        &mut self,
        conn: &mut BufferedConnection,
    ) -> Result<(), Socks5ProxyError> {
        info!("Handling SOCKS5 connection");

        // 1. 握手阶段
        let selected_method = self.handshake(conn).await?;

        // 2. 认证阶段 (only if method 0x02 selected)
        if selected_method == 0x02 {
            self.authenticate(conn).await?;
        }

        // 3. 请求阶段
        let target_addr = self.handle_request(conn).await?;

        // 4. 连接目标服务器
        let target_stream = TcpStream::connect(target_addr)
            .await
            .map_err(Socks5ProxyError::ConnectTargetFailed)?;

        info!("Connected to target server: {}", target_addr);

        // 5. 发送连接成功响应
        self.send_connection_success(conn).await?;

        // 6. 数据转发
        // 使用与客户端连接相同的缓冲区大小
        let buffer_size = conn.buffer_size();
        let mut target_conn = BufferedConnection::new(target_stream, buffer_size);
        Forwarder::forward_between(conn, &mut target_conn)
            .await
            .map_err(Socks5ProxyError::IoError)?;

        Ok(())
    }

    /// 握手阶段
    async fn handshake(&mut self, conn: &mut BufferedConnection) -> Result<u8, Socks5ProxyError> {
        // 确保有足够的数据
        while conn.available_bytes() < 2 {
            if conn.read().await? == 0 {
                return Err(Socks5ProxyError::ConnectionClosed("handshake"));
            }
        }

        // 读取版本号和认证方法数量
        let version = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0],
            None => return Err(Socks5ProxyError::ConnectionClosed("handshake")),
        };
        let nmethods = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0] as usize,
            None => return Err(Socks5ProxyError::ConnectionClosed("handshake")),
        };

        // 验证版本号
        if version != 0x05 {
            return Err(Socks5ProxyError::InvalidVersion);
        }

        // 确保有足够的数据
        while conn.available_bytes() < nmethods {
            if conn.read().await? == 0 {
                return Err(Socks5ProxyError::ConnectionClosed("handshake"));
            }
        }

        // 读取所有认证方法
        let methods = match conn.read_from_buffer(nmethods) {
            Some(data) => data,
            None => return Err(Socks5ProxyError::ConnectionClosed("handshake")),
        };

        // 检查是否支持无认证或用户名密码认证
        let mut selected_method = 0xFF; // 不支持的方法

        if methods.contains(&0x00) && !self.auth_manager.has_users() {
            // 无认证
            selected_method = 0x00;
            info!("Selected SOCKS5 no authentication");
        } else if methods.contains(&0x02) && self.auth_manager.has_users() {
            // 用户名密码认证
            selected_method = 0x02;
            info!("Selected SOCKS5 username/password authentication");
        }

        // 发送选择的认证方法
        let response = vec![0x05, selected_method];
        conn.write(&response).await?;

        if selected_method == 0xFF {
            return Err(Socks5ProxyError::NoSupportedAuthMethod);
        }

        Ok(selected_method)
    }

    /// 认证阶段
    async fn authenticate(
        &mut self,
        conn: &mut BufferedConnection,
    ) -> Result<(), Socks5ProxyError> {
        // 确保有足够的数据
        while conn.available_bytes() < 2 {
            if conn.read().await? == 0 {
                return Err(Socks5ProxyError::ConnectionClosed("authentication"));
            }
        }

        // 读取认证版本和方法
        let auth_version = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0],
            None => return Err(Socks5ProxyError::ConnectionClosed("authentication")),
        };
        let auth_method = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0],
            None => return Err(Socks5ProxyError::ConnectionClosed("authentication")),
        };

        if auth_version != 0x01 || auth_method != 0x02 {
            return Err(Socks5ProxyError::InvalidAuthMethod);
        }

        // 读取用户名长度
        while conn.available_bytes() < 1 {
            if conn.read().await? == 0 {
                return Err(Socks5ProxyError::ConnectionClosed("authentication"));
            }
        }

        let username_len = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0] as usize,
            None => return Err(Socks5ProxyError::ConnectionClosed("authentication")),
        };

        // 读取用户名
        while conn.available_bytes() < username_len {
            if conn.read().await? == 0 {
                return Err(Socks5ProxyError::ConnectionClosed("authentication"));
            }
        }

        let username = match conn.read_from_buffer(username_len) {
            Some(data) => String::from_utf8(data)?,
            None => return Err(Socks5ProxyError::ConnectionClosed("authentication")),
        };

        // 读取密码长度
        while conn.available_bytes() < 1 {
            if conn.read().await? == 0 {
                return Err(Socks5ProxyError::ConnectionClosed("authentication"));
            }
        }

        let password_len = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0] as usize,
            None => return Err(Socks5ProxyError::ConnectionClosed("authentication")),
        };

        // 读取密码
        while conn.available_bytes() < password_len {
            if conn.read().await? == 0 {
                return Err(Socks5ProxyError::ConnectionClosed("authentication"));
            }
        }

        let password = match conn.read_from_buffer(password_len) {
            Some(data) => String::from_utf8(data)?,
            None => return Err(Socks5ProxyError::ConnectionClosed("authentication")),
        };

        // 验证用户名和密码
        let auth_success = self.auth_manager.authenticate(&username, &password)?;

        // 发送认证结果
        let response = if auth_success {
            vec![0x01, 0x00]
        } else {
            vec![0x01, 0x01]
        };

        conn.write(&response).await?;

        if !auth_success {
            return Err(Socks5ProxyError::AuthenticationFailed(
                AuthError::AuthenticationFailed,
            ));
        }

        info!("User {} authenticated successfully", username);
        Ok(())
    }

    /// 处理请求
    async fn handle_request(
        &mut self,
        conn: &mut BufferedConnection,
    ) -> Result<SocketAddr, Socks5ProxyError> {
        // 确保有足够的数据
        while conn.available_bytes() < 4 {
            if conn.read().await? == 0 {
                return Err(Socks5ProxyError::ConnectionClosed("request"));
            }
        }

        // 读取版本、命令、保留字段和地址类型
        let version = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0],
            None => return Err(Socks5ProxyError::ConnectionClosed("request")),
        };
        let command = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0],
            None => return Err(Socks5ProxyError::ConnectionClosed("request")),
        };
        let _reserved = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0],
            None => return Err(Socks5ProxyError::ConnectionClosed("request")),
        };
        let addr_type = match conn.read_from_buffer(1) {
            Some(bytes) => bytes[0],
            None => return Err(Socks5ProxyError::ConnectionClosed("request")),
        };

        // 验证版本和命令
        if version != 0x05 {
            return Err(Socks5ProxyError::InvalidVersion);
        }

        if command != 0x01 {
            // 只支持CONNECT命令
            return Err(Socks5ProxyError::UnsupportedCommand);
        }

        // 解析目标地址
        let target_addr = match addr_type {
            0x01 => {
                // IPv4地址
                while conn.available_bytes() < 6 {
                    if conn.read().await? == 0 {
                        return Err(Socks5ProxyError::ConnectionClosed("request"));
                    }
                }

                let addr_bytes = match conn.read_from_buffer(4) {
                    Some(data) => data,
                    None => return Err(Socks5ProxyError::ConnectionClosed("request")),
                };
                let port_bytes = match conn.read_from_buffer(2) {
                    Some(data) => data,
                    None => return Err(Socks5ProxyError::ConnectionClosed("request")),
                };
                let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);

                SocketAddr::new(
                    std::net::Ipv4Addr::new(
                        addr_bytes[0],
                        addr_bytes[1],
                        addr_bytes[2],
                        addr_bytes[3],
                    )
                    .into(),
                    port,
                )
            }
            0x03 => {
                // 域名
                while conn.available_bytes() < 1 {
                    if conn.read().await? == 0 {
                        return Err(Socks5ProxyError::ConnectionClosed("request"));
                    }
                }

                let domain_len = match conn.read_from_buffer(1) {
                    Some(bytes) => bytes[0] as usize,
                    None => return Err(Socks5ProxyError::ConnectionClosed("request")),
                };

                while conn.available_bytes() < (domain_len + 2) {
                    if conn.read().await? == 0 {
                        return Err(Socks5ProxyError::ConnectionClosed("request"));
                    }
                }

                let domain = match conn.read_from_buffer(domain_len) {
                    Some(data) => String::from_utf8(data)?,
                    None => return Err(Socks5ProxyError::ConnectionClosed("request")),
                };
                let port_bytes = match conn.read_from_buffer(2) {
                    Some(data) => data,
                    None => return Err(Socks5ProxyError::ConnectionClosed("request")),
                };
                let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);

                // 解析域名
                (domain.as_str(), port)
                    .to_socket_addrs()
                    .map_err(|_| Socks5ProxyError::AddressResolutionFailed(domain.clone()))?
                    .next()
                    .ok_or(Socks5ProxyError::AddressResolutionFailed(domain))?
            }
            0x04 => {
                // IPv6地址
                while conn.available_bytes() < 18 {
                    if conn.read().await? == 0 {
                        return Err(Socks5ProxyError::ConnectionClosed("request"));
                    }
                }

                let addr_bytes = match conn.read_from_buffer(16) {
                    Some(data) => data,
                    None => return Err(Socks5ProxyError::ConnectionClosed("request")),
                };
                let port_bytes = match conn.read_from_buffer(2) {
                    Some(data) => data,
                    None => return Err(Socks5ProxyError::ConnectionClosed("request")),
                };
                let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);

                SocketAddr::new(
                    std::net::Ipv6Addr::new(
                        u16::from_be_bytes([addr_bytes[0], addr_bytes[1]]),
                        u16::from_be_bytes([addr_bytes[2], addr_bytes[3]]),
                        u16::from_be_bytes([addr_bytes[4], addr_bytes[5]]),
                        u16::from_be_bytes([addr_bytes[6], addr_bytes[7]]),
                        u16::from_be_bytes([addr_bytes[8], addr_bytes[9]]),
                        u16::from_be_bytes([addr_bytes[10], addr_bytes[11]]),
                        u16::from_be_bytes([addr_bytes[12], addr_bytes[13]]),
                        u16::from_be_bytes([addr_bytes[14], addr_bytes[15]]),
                    )
                    .into(),
                    port,
                )
            }
            _ => {
                return Err(Socks5ProxyError::InvalidAddressType);
            }
        };

        Ok(target_addr)
    }

    /// 发送连接成功响应
    async fn send_connection_success(
        &mut self,
        conn: &mut BufferedConnection,
    ) -> Result<(), Socks5ProxyError> {
        // 响应格式: 版本(1字节) + 响应码(1字节) + 保留字段(1字节) + 地址类型(1字节) + 绑定地址(可变) + 绑定端口(2字节)
        // 这里使用0.0.0.0:0作为绑定地址和端口
        let response = vec![0x05, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

        conn.write(&response).await?;
        info!("Sent connection success response");
        Ok(())
    }
}
