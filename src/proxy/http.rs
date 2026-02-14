use base64::{engine::general_purpose, Engine as _};
use log::info;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncWriteExt;

use crate::common::auth::AuthManager;
use crate::net::conn::BufferedConnection;
use crate::proxy::forward;

/// HTTP proxy error types
#[derive(Error, Debug)]
pub enum HttpProxyError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Invalid HTTP request: {0}")]
    InvalidRequest(String),
    #[error("Proxy authentication required")]
    ProxyAuthRequired,
    #[error("Authentication failed")]
    AuthenticationFailed(#[from] crate::common::auth::AuthError),
    #[error("Unsupported HTTP method: {0}")]
    UnsupportedMethod(String),
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("Connection error: {0}")]
    ConnectError(#[from] crate::proxy::forward::ConnectError),
    #[error("Invalid UTF-8 data: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("Invalid base64 encoding: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
}

/// A single HTTP header preserving its original name casing.
struct HttpHeader {
    /// Original header name (preserves case, e.g. "Content-Type")
    name: String,
    /// Lowercased name used for lookups
    name_lower: String,
    /// Header value
    value: String,
}

struct HttpRequest {
    method: String,
    path: String,
    version: String,
    /// Headers stored in order, preserving original case.
    headers: Vec<HttpHeader>,
    body: Vec<u8>,
}

impl HttpRequest {
    /// Lookup a header value by name (case-insensitive).
    fn get_header(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        self.headers
            .iter()
            .find(|h| h.name_lower == lower)
            .map(|h| h.value.as_str())
    }
}

const CONNECT_OK: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";
const PROXY_AUTH_REQUIRED: &[u8] = b"HTTP/1.1 407 Proxy Authentication Required\r\n\
    Proxy-Authenticate: Basic realm=\"Proxy\"\r\n\
    Content-Length: 0\r\n\r\n";

/// HTTP proxy implementation supporting CONNECT and standard HTTP methods
pub struct HttpProxy {
    auth_manager: Arc<AuthManager>,
    buffer_size: usize,
    connect_timeout: Duration,
}

impl HttpProxy {
    /// Creates a new HTTP proxy instance
    pub fn new(
        auth_manager: Arc<AuthManager>,
        buffer_size: usize,
        connect_timeout: Duration,
    ) -> Self {
        HttpProxy {
            auth_manager,
            buffer_size,
            connect_timeout,
        }
    }

    /// Handles an incoming HTTP proxy connection
    pub async fn handle_connection(
        &self,
        conn: &mut BufferedConnection,
    ) -> Result<(), HttpProxyError> {
        let request = self.parse_request(conn).await?;

        if self.auth_manager.has_users() {
            self.authenticate(conn, &request).await?;
        }

        match request.method.as_str() {
            "CONNECT" => self.handle_connect(conn, &request).await?,
            "GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" | "PATCH" => {
                self.handle_http_request(conn, &request).await?
            }
            _ => {
                return Err(HttpProxyError::UnsupportedMethod(request.method.clone()));
            }
        }

        Ok(())
    }

    /// Parses an HTTP request from the buffered connection
    async fn parse_request(
        &self,
        conn: &mut BufferedConnection,
    ) -> Result<HttpRequest, HttpProxyError> {
        let request_line = conn.read_line().await?;
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(HttpProxyError::InvalidRequest(
                "Invalid HTTP request line".to_string(),
            ));
        }

        let method = parts[0].to_string();
        let path = parts[1].to_string();
        let version = parts[2].to_string();

        let mut headers = Vec::new();
        loop {
            let line = conn.read_line().await?;
            if line.is_empty() {
                break;
            }
            if let Some(colon_pos) = line.find(':') {
                let name = line[..colon_pos].trim().to_string();
                let name_lower = name.to_lowercase();
                let value = line[colon_pos + 1..].trim().to_string();
                headers.push(HttpHeader {
                    name,
                    name_lower,
                    value,
                });
            }
        }

        let body = if let Some(content_length) = headers
            .iter()
            .find(|h| h.name_lower == "content-length")
            .map(|h| h.value.as_str())
        {
            let len = content_length.parse::<usize>().map_err(|_| {
                HttpProxyError::InvalidRequest("Invalid Content-Length".to_string())
            })?;
            conn.read_exact_bytes(len).await?
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

    /// Handles proxy authentication
    async fn authenticate(
        &self,
        conn: &mut BufferedConnection,
        request: &HttpRequest,
    ) -> Result<(), HttpProxyError> {
        if let Some(auth_header) = request.get_header("proxy-authorization") {
            if let Some(encoded) = auth_header.strip_prefix("Basic ") {
                let decoded = general_purpose::STANDARD.decode(encoded)?;
                let credentials = String::from_utf8(decoded)?;

                if let Some(colon_pos) = credentials.find(':') {
                    let username = &credentials[..colon_pos];
                    let password = &credentials[colon_pos + 1..];

                    match self.auth_manager.authenticate(username, password).await {
                        Ok(true) => return Ok(()),
                        Ok(false) => {
                            // Wrong credentials – fall through to send 407
                        }
                        Err(e) => {
                            conn.write(PROXY_AUTH_REQUIRED).await?;
                            return Err(HttpProxyError::AuthenticationFailed(e));
                        }
                    }
                }
            }
        }

        conn.write(PROXY_AUTH_REQUIRED).await?;
        Err(HttpProxyError::ProxyAuthRequired)
    }

    /// Handles CONNECT method to create a TCP tunnel
    async fn handle_connect(
        &self,
        conn: &mut BufferedConnection,
        request: &HttpRequest,
    ) -> Result<(), HttpProxyError> {
        let target_stream =
            forward::connect_with_timeout(&request.path, self.connect_timeout).await?;

        conn.write(CONNECT_OK).await?;
        info!("CONNECT tunnel to {}", request.path);

        let mut target_conn = BufferedConnection::new(target_stream, self.buffer_size);
        forward::forward_bidirectional(conn, &mut target_conn).await?;

        Ok(())
    }

    /// Handles standard HTTP requests (GET, POST, PUT, etc.)
    async fn handle_http_request(
        &self,
        conn: &mut BufferedConnection,
        request: &HttpRequest,
    ) -> Result<(), HttpProxyError> {
        let url = url::Url::parse(&request.path)?;
        let host = url
            .host_str()
            .ok_or_else(|| HttpProxyError::InvalidRequest("No host in URL".to_string()))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| HttpProxyError::InvalidRequest("No port in URL".to_string()))?;

        let target_addr = format!("{}:{}", host, port);
        let target_stream =
            forward::connect_with_timeout(&target_addr, self.connect_timeout).await?;

        let mut target_conn = BufferedConnection::new(target_stream, self.buffer_size);

        let relative_path = match url.query() {
            None => url.path().to_string(),
            Some(q) => format!("{}?{}", url.path(), q),
        };

        let mut request_data = Vec::new();
        request_data.extend_from_slice(
            format!(
                "{} {} {}\r\n",
                request.method, relative_path, request.version
            )
            .as_bytes(),
        );

        // Forward headers preserving original order and case,
        // skipping hop-by-hop proxy headers.
        for header in &request.headers {
            if !header.name_lower.starts_with("proxy-") && header.name_lower != "connection" {
                request_data
                    .extend_from_slice(format!("{}: {}\r\n", header.name, header.value).as_bytes());
            }
        }
        request_data.extend_from_slice(b"Connection: close\r\n\r\n");

        if !request.body.is_empty() {
            request_data.extend_from_slice(&request.body);
        }

        target_conn.write(&request_data).await?;
        info!("HTTP {} {}", request.method, request.path);

        // For non-CONNECT requests the full request has already been sent.
        // Only copy the response back (target → client) instead of using
        // bidirectional forwarding which could mis-forward pipelined data.
        tokio::io::copy(&mut target_conn, conn).await?;
        conn.shutdown().await?;

        Ok(())
    }
}
