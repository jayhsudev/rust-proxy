use base64::{engine::general_purpose, Engine as _};
use std::io::{Error, ErrorKind};
use std::net::ToSocketAddrs;
use std::sync::Arc;
use tokio::net::TcpStream;

use crate::common::auth::AuthManager;
use crate::net::conn::BufferedConnection;
use crate::proxy::forward::Forwarder;

/// HTTP请求结构
#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    version: String,
    headers: std::collections::HashMap<String, String>,
    body: Vec<u8>,
}

/// HTTP代理
pub struct HttpProxy {
    /// 身份验证管理器
    auth_manager: Arc<AuthManager>,
}

impl HttpProxy {
    /// 创建新的HTTP代理
    pub fn new(auth_manager: Arc<AuthManager>) -> Self {
        HttpProxy { auth_manager }
    }

    /// 处理HTTP连接
    pub async fn handle_connection(
        &mut self,
        conn: &mut BufferedConnection,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // 1. 解析HTTP请求
        let request = self.parse_request(conn).await?;

        // 2. 处理身份验证
        if self.auth_manager.has_users() {
            self.authenticate(conn, &request).await?;
        }

        // 3. 处理请求
        match request.method.as_str() {
            "CONNECT" => {
                // 处理HTTPS CONNECT请求
                self.handle_connect(conn, &request).await?;
            }
            _ => {
                // 处理普通HTTP请求
                self.handle_http_request(conn, &request).await?;
            }
        }

        Ok(())
    }

    /// 解析HTTP请求
    async fn parse_request(
        &mut self,
        conn: &mut BufferedConnection,
    ) -> Result<HttpRequest, Box<dyn std::error::Error>> {
        // 读取请求行
        let mut request_line = String::new();
        loop {
            // 确保有数据可读
            if conn.available_bytes() == 0 && conn.read().await? == 0 {
                return Err("Connection closed during request parsing".into());
            }

            // 读取一个字节
            let byte = match conn.read_from_buffer(1) {
                Some(bytes) => bytes[0],
                None => return Err("Connection closed during request parsing".into()),
            };
            let c = byte as char;

            if c == '\r' {
                // 检查下一个字符是否是\n
                if conn.available_bytes() == 0 && conn.read().await? == 0 {
                    return Err("Connection closed during request parsing".into());
                }
                let next_byte = match conn.read_from_buffer(1) {
                    Some(bytes) => bytes[0],
                    None => return Err("Connection closed during request parsing".into()),
                };
                if next_byte as char == '\n' {
                    // 找到行结束符\r\n
                    break;
                } else {
                    // 不是\n，把两个字符都加入
                    request_line.push(c);
                    request_line.push(next_byte as char);
                }
            } else if c == '\n' {
                // 只有\n，也认为是行结束（兼容处理）
                break;
            } else {
                request_line.push(c);
            }
        }

        // 解析请求行
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 3 {
            return Err("Invalid HTTP request line".into());
        }

        let method = parts[0].to_string();
        let path = parts[1].to_string();
        let version = parts[2].to_string();

        // 解析请求头
        let mut headers = std::collections::HashMap::new();
        loop {
            let mut header_line = String::new();

            loop {
                // 确保有数据可读
                if conn.available_bytes() == 0 && conn.read().await? == 0 {
                    return Err("Connection closed during header parsing".into());
                }

                // 读取一个字节
                let byte = match conn.read_from_buffer(1) {
                    Some(bytes) => bytes[0],
                    None => return Err("Connection closed during header parsing".into()),
                };
                let c = byte as char;

                if c == '\r' {
                    // 检查下一个字符是否是\n
                    if conn.available_bytes() == 0 && conn.read().await? == 0 {
                        return Err("Connection closed during header parsing".into());
                    }
                    let next_byte = match conn.read_from_buffer(1) {
                        Some(bytes) => bytes[0],
                        None => return Err("Connection closed during header parsing".into()),
                    };
                    if next_byte as char == '\n' {
                        // 找到行结束符\r\n
                        break;
                    } else {
                        // 不是\n，把两个字符都加入
                        header_line.push(c);
                        header_line.push(next_byte as char);
                    }
                } else if c == '\n' {
                    // 只有\n，也认为是行结束（兼容处理）
                    break;
                } else {
                    header_line.push(c);
                }
            }

            // 检查是否是头部结束符（空行）
            if header_line.is_empty() {
                break;
            }

            // 解析头部行
            if let Some(colon_pos) = header_line.find(':') {
                let name = header_line[..colon_pos].trim().to_lowercase();
                let value = header_line[colon_pos + 1..].trim().to_string();
                headers.insert(name, value);
            }
        }

        // 读取请求体（如果有）
        let body = if let Some(content_length) = headers.get("content-length") {
            let len = content_length.parse::<usize>()?;
            let mut body = Vec::with_capacity(len);

            while body.len() < len {
                if conn.has_data() {
                    let available = conn.available_bytes();
                    let take = std::cmp::min(available, len - body.len());
                    let data = match conn.read_from_buffer(take) {
                        Some(bytes) => bytes,
                        None => break,
                    };
                    body.extend_from_slice(&data);
                } else if conn.read().await? == 0 {
                    break;
                }
            }

            body
        } else {
            Vec::new()
        };

        Ok(HttpRequest {
            method,
            path,
            version,
            headers,
            body,
        })
    }

    /// 处理身份验证
    async fn authenticate(
        &mut self,
        conn: &mut BufferedConnection,
        request: &HttpRequest,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // 检查Authorization头
        if let Some(auth_header) = request.headers.get("authorization") {
            if let Some(encoded) = auth_header.strip_prefix("Basic ") {
                let decoded = general_purpose::STANDARD.decode(encoded)?;
                let credentials = String::from_utf8(decoded)?;

                if let Some(colon_pos) = credentials.find(':') {
                    let username = &credentials[..colon_pos];
                    let password = &credentials[colon_pos + 1..];

                    if self.auth_manager.authenticate(username, password)? {
                        return Ok(());
                    }
                }
            }
        }

        // 认证失败，发送407响应
        let response = b"HTTP/1.1 407 Proxy Authentication Required\r\n"
            .iter()
            .chain(b"Proxy-Authenticate: Basic realm=\"WProxy\"\r\n")
            .chain(b"Content-Length: 0\r\n")
            .chain(b"\r\n")
            .cloned()
            .collect::<Vec<u8>>();

        conn.write(&response).await?;
        Err("Proxy authentication required".into())
    }

    /// 处理CONNECT请求
    async fn handle_connect(
        &mut self,
        conn: &mut BufferedConnection,
        request: &HttpRequest,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // 解析目标地址
        let target_addr =
            request.path.to_socket_addrs()?.next().ok_or_else(|| {
                Error::new(ErrorKind::NotFound, "Could not resolve target address")
            })?;

        // 连接目标服务器
        let target_stream = TcpStream::connect(target_addr).await.map_err(|e| {
            Error::new(
                ErrorKind::ConnectionRefused,
                format!("Failed to connect to target: {}", e),
            )
        })?;

        // 发送连接成功响应
        let response = b"HTTP/1.1 200 Connection Established\r\n"
            .iter()
            .chain(b"Content-Length: 0\r\n")
            .chain(b"\r\n")
            .cloned()
            .collect::<Vec<u8>>();

        conn.write(&response).await?;

        // 数据转发
        let mut target_conn = BufferedConnection::new(target_stream, 4096);
        Forwarder::forward_between(conn, &mut target_conn).await?;

        Ok(())
    }

    /// 处理普通HTTP请求
    async fn handle_http_request(
        &mut self,
        conn: &mut BufferedConnection,
        request: &HttpRequest,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // 解析URL
        let url = url::Url::parse(&request.path)?;
        let host = url
            .host_str()
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "No host in URL"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "No port in URL"))?;

        // 连接目标服务器
        let target_addr = (host, port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| Error::new(ErrorKind::NotFound, "Could not resolve target address"))?;

        let target_stream = TcpStream::connect(target_addr).await.map_err(|e| {
            Error::new(
                ErrorKind::ConnectionRefused,
                format!("Failed to connect to target: {}", e),
            )
        })?;

        // 创建目标连接
        let mut target_conn = BufferedConnection::new(target_stream, 4096);

        // 重写请求行（使用相对路径）
        let relative_path = if url.path() == "/" && url.query().is_none() {
            "/".to_string()
        } else if url.query().is_none() {
            url.path().to_string()
        } else {
            format!("{}?{}", url.path(), url.query().unwrap())
        };

        let request_line = format!(
            "{} {} {}\r\n",
            request.method, relative_path, request.version
        );
        target_conn.write_to_buffer(request_line.as_bytes());

        // 转发请求头（移除Proxy-*头）
        for (name, value) in &request.headers {
            if !name.starts_with("proxy-") && name != "connection" {
                let header_line = format!("{}: {}\r\n", name, value);
                target_conn.write_to_buffer(header_line.as_bytes());
            }
        }

        // 添加Connection: close头
        target_conn.write_to_buffer(b"Connection: close\r\n");

        // 结束请求头
        target_conn.write_to_buffer(b"\r\n");

        // 转发请求体
        if !request.body.is_empty() {
            target_conn.write_to_buffer(&request.body);
        }

        // 刷新缓冲区
        target_conn.flush().await?;

        // 数据转发
        Forwarder::forward_between(&mut target_conn, conn).await?;

        Ok(())
    }
}
