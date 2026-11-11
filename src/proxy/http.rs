use base64::{Engine as _, engine::general_purpose};
use log::{debug, warn};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::time::timeout;

use crate::common::auth::AuthManager;
use crate::common::metrics::Metrics;
use crate::net::conn::BufferedConnection;
use crate::proxy::forward::{self, Upstream};
use crate::proxy::http1::{self, Header, MAX_HEAD_BYTES, Parsed};

#[derive(Error, Debug)]
pub enum HttpProxyError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Invalid HTTP request: {0}")]
    InvalidRequest(String),
    #[error("Request head exceeds {MAX_HEAD_BYTES} bytes")]
    HeadTooLarge,
    #[error("Proxy authentication required")]
    ProxyAuthRequired,
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(#[from] crate::common::auth::AuthError),
    #[error("Unsupported HTTP method: {0}")]
    UnsupportedMethod(String),
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("Connection error: {0}")]
    ConnectError(#[from] forward::ConnectError),
    #[error("Invalid UTF-8 data: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("Invalid base64 encoding: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
    #[error("Client request timed out")]
    RequestTimeout,
}

struct RequestHead {
    method: String,
    target: String,
    version_minor: u8,
    headers: Vec<Header>,
}

/// How the request body is framed on the wire.
#[derive(Clone, Copy)]
enum BodyKind {
    Empty,
    Length(usize),
    Chunked,
}

impl RequestHead {
    fn get(&self, lower_name: &str) -> Option<&str> {
        http1::get_str(&self.headers, lower_name)
    }

    fn is_chunked(&self) -> bool {
        http1::has_token(&self.headers, "transfer-encoding", "chunked")
    }

    fn expects_continue(&self) -> bool {
        self.get("expect")
            .map(|value| value.eq_ignore_ascii_case("100-continue"))
            .unwrap_or(false)
    }

    fn content_length(&self) -> Result<Option<usize>, HttpProxyError> {
        match self.get("content-length") {
            Some(value) => value
                .parse::<usize>()
                .map(Some)
                .map_err(|_| invalid_request("Invalid Content-Length")),
            None => Ok(None),
        }
    }

    /// Determines the body framing, rejecting ambiguous requests (request smuggling).
    fn body_kind(&self) -> Result<BodyKind, HttpProxyError> {
        let content_length = self.content_length()?;

        if self.is_chunked() {
            if content_length.is_some() {
                return Err(invalid_request(
                    "Both Content-Length and Transfer-Encoding are present",
                ));
            }
            return Ok(BodyKind::Chunked);
        }

        match content_length {
            None | Some(0) => Ok(BodyKind::Empty),
            Some(len) => Ok(BodyKind::Length(len)),
        }
    }
}

const CONNECT_OK: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";
const CONTINUE_100: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";
const PROXY_AUTH_REQUIRED: &[u8] = b"HTTP/1.1 407 Proxy Authentication Required\r\n\
    Proxy-Authenticate: Basic realm=\"Proxy\"\r\n\
    Content-Length: 0\r\n\
    Connection: close\r\n\r\n";
const BAD_REQUEST: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const REQUEST_TIMEOUT: &[u8] =
    b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HEADERS_TOO_LARGE: &[u8] =
    b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const NOT_IMPLEMENTED: &[u8] =
    b"HTTP/1.1 501 Not Implemented\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const BAD_GATEWAY: &[u8] =
    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const GATEWAY_TIMEOUT: &[u8] =
    b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

const SUPPORTED_METHODS: [&str; 7] = ["GET", "POST", "PUT", "DELETE", "HEAD", "OPTIONS", "PATCH"];

pub struct HttpProxy {
    auth_manager: Arc<AuthManager>,
    upstream: Arc<Upstream>,
    metrics: Arc<Metrics>,
}

impl HttpProxy {
    pub fn new(
        auth_manager: Arc<AuthManager>,
        upstream: Arc<Upstream>,
        metrics: Arc<Metrics>,
    ) -> Self {
        HttpProxy {
            auth_manager,
            upstream,
            metrics,
        }
    }

    pub async fn handle_connection(
        &self,
        mut conn: BufferedConnection,
    ) -> Result<(), HttpProxyError> {
        let handshake_timeout = self.upstream.options().handshake_timeout;

        let head = match timeout(handshake_timeout, self.read_head(&mut conn)).await {
            Ok(Ok(head)) => head,
            Ok(Err(e)) => {
                let response = if matches!(e, HttpProxyError::HeadTooLarge) {
                    HEADERS_TOO_LARGE
                } else {
                    BAD_REQUEST
                };
                let _ = conn.write(response).await;
                return Err(e);
            }
            Err(_) => {
                let _ = conn.write(REQUEST_TIMEOUT).await;
                return Err(HttpProxyError::RequestTimeout);
            }
        };

        if self.auth_manager.has_users() {
            self.authenticate(&mut conn, &head).await?;
        }

        if head.method == "CONNECT" {
            return self.handle_connect(conn, &head).await;
        }

        if !SUPPORTED_METHODS.contains(&head.method.as_str()) {
            let _ = conn.write(NOT_IMPLEMENTED).await;
            return Err(HttpProxyError::UnsupportedMethod(head.method.clone()));
        }

        let body_kind = match head.body_kind() {
            Ok(kind) => kind,
            Err(e) => {
                let _ = conn.write(BAD_REQUEST).await;
                return Err(e);
            }
        };

        if head.expects_continue() {
            conn.write(CONTINUE_100).await?;
        }

        self.forward_request(conn, &head, body_kind).await
    }

    /// Parses the request head with `httparse`, keeping unparsed bytes buffered.
    async fn read_head(
        &self,
        conn: &mut BufferedConnection,
    ) -> Result<RequestHead, HttpProxyError> {
        loop {
            let available = conn.buffer_len();

            if available > 0 {
                let parsed = {
                    let buffer = conn
                        .buffer_slice(available)
                        .ok_or_else(|| invalid_request("buffer unavailable"))?;
                    http1::parse_request(buffer)
                };

                match parsed {
                    Ok(Parsed::Complete(parts)) => {
                        let consumed = parts.consumed;
                        let all = conn
                            .read_from_buffer(available)
                            .ok_or_else(|| invalid_request("buffer underflow"))?;

                        if consumed < all.len() {
                            conn.unread(&all[consumed..]);
                        }

                        return Ok(RequestHead {
                            method: parts.method,
                            target: parts.target,
                            version_minor: parts.minor_version,
                            headers: parts.headers,
                        });
                    }
                    Ok(Parsed::Partial) => {
                        if available > http1::MAX_HEAD_BYTES {
                            return Err(HttpProxyError::HeadTooLarge);
                        }
                    }
                    Err(http1::ParseError::TooManyHeaders) => {
                        return Err(HttpProxyError::HeadTooLarge);
                    }
                    Err(e) => return Err(invalid_request(e.to_string())),
                }
            }

            if conn.read().await? == 0 {
                return Err(invalid_request("connection closed before the request head"));
            }
        }
    }

    async fn authenticate(
        &self,
        conn: &mut BufferedConnection,
        head: &RequestHead,
    ) -> Result<(), HttpProxyError> {
        let credentials = head
            .get("proxy-authorization")
            .and_then(|value| value.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("basic"))
            .map(|(_, encoded)| encoded.trim());

        if let Some(encoded) = credentials {
            let decoded = general_purpose::STANDARD.decode(encoded)?;
            let credentials = String::from_utf8(decoded)?;

            if let Some(colon_pos) = credentials.find(':') {
                let username = &credentials[..colon_pos];
                let password = &credentials[colon_pos + 1..];

                match self.auth_manager.authenticate(username, password).await {
                    Ok(true) => {
                        debug!("HTTP proxy user '{}' authenticated", username);
                        return Ok(());
                    }
                    Ok(false) => {
                        // Counted once at the bottom of this function.
                        warn!("HTTP proxy authentication failed for '{}'", username);
                    }
                    Err(e) => {
                        self.metrics.auth_failure();
                        conn.write(PROXY_AUTH_REQUIRED).await?;
                        return Err(HttpProxyError::AuthenticationFailed(e));
                    }
                }
            }
        }

        self.metrics.auth_failure();
        conn.write(PROXY_AUTH_REQUIRED).await?;
        Err(HttpProxyError::ProxyAuthRequired)
    }

    async fn handle_connect(
        &self,
        mut conn: BufferedConnection,
        head: &RequestHead,
    ) -> Result<(), HttpProxyError> {
        let target = head.target.trim();

        if target.is_empty() || !target.contains(':') {
            let _ = conn.write(BAD_REQUEST).await;
            return Err(invalid_request("CONNECT target must be host:port"));
        }

        let target_stream = match self.upstream.connect(target).await {
            Ok(stream) => stream,
            Err(e) => {
                self.metrics.upstream_error();
                let _ = conn.write(connect_error_response(&e)).await;
                return Err(HttpProxyError::ConnectError(e));
            }
        };

        let buffer_size = self.upstream.options().buffer_size;
        let target_conn = BufferedConnection::new(target_stream, buffer_size);

        conn.write(CONNECT_OK).await?;
        debug!("HTTP CONNECT tunnel to {}", target);

        match forward::relay(conn, target_conn, self.upstream.options().relay_buffer_size).await {
            Ok((up, down)) => {
                self.metrics.add_bytes(up, down);
                Ok(())
            }
            Err(e) => {
                self.metrics.upstream_error();
                Err(HttpProxyError::IoError(e))
            }
        }
    }

    /// Streams the request upstream and relays the response back.
    ///
    /// Both directions run concurrently, so an upstream that answers before
    /// consuming the request body (413, 401, redirects…) cannot deadlock the
    /// proxy. When the response is delimited and the request body was fully
    /// sent, the upstream connection is returned to the pool for reuse.
    async fn forward_request(
        &self,
        mut conn: BufferedConnection,
        head: &RequestHead,
        body_kind: BodyKind,
    ) -> Result<(), HttpProxyError> {
        let (host, port, path) = resolve_target(head)?;
        let target_addr = forward::join_host_port(&host, port);
        let pooling = self.upstream.pooling_enabled();
        let request_data = build_request_head(head, &path, pooling);

        let mut reused = false;

        let (mut upstream_read, mut upstream_write) =
            match self.upstream.take_pooled(&target_addr).await {
                Some(pooled) => {
                    reused = true;
                    pooled.into_split()
                }
                None => match self.upstream.connect(&target_addr).await {
                    Ok(stream) => stream.into_split(),
                    Err(e) => {
                        self.metrics.upstream_error();
                        self.metrics.pool_miss();
                        let _ = conn.write(connect_error_response(&e)).await;
                        return Err(HttpProxyError::ConnectError(e));
                    }
                },
            };

        if let Err(error) = upstream_write.write_all(&request_data).await {
            if !reused {
                self.metrics.upstream_error();
                let _ = conn.write(BAD_GATEWAY).await;
                return Err(HttpProxyError::IoError(error));
            }

            // A pooled connection may have been closed by the peer in the
            // meantime. Nothing of the body has been consumed yet, so retrying
            // once on a fresh connection is safe.
            debug!(
                "Pooled upstream {} is stale ({}), reconnecting",
                target_addr, error
            );

            match self.upstream.connect(&target_addr).await {
                Ok(fresh) => {
                    let (read_half, write_half) = fresh.into_split();
                    upstream_read = read_half;
                    upstream_write = write_half;

                    if let Err(error) = upstream_write.write_all(&request_data).await {
                        self.metrics.upstream_error();
                        let _ = conn.write(BAD_GATEWAY).await;
                        return Err(HttpProxyError::IoError(error));
                    }

                    // The pooled connection was unusable, so this request needed
                    // a fresh one.
                    reused = false;
                }
                Err(e) => {
                    self.metrics.upstream_error();
                    let _ = conn.write(connect_error_response(&e)).await;
                    return Err(HttpProxyError::ConnectError(e));
                }
            }
        }

        // Account the outcome of upstream acquisition now that it succeeded.
        if reused {
            self.metrics.pool_hit();
        } else {
            self.metrics.pool_miss();
        }

        self.metrics.http_request();
        debug!("HTTP {} {}", head.method, head.target);

        let (mut client_read, mut client_write) = conn.into_split();
        let mut upstream_reader = BufReader::new(upstream_read);

        let mut body_bytes = 0u64;
        let mut body_ok = false;
        let mut outcome: Option<http1::RelayOutcome> = None;

        tokio::select! {
            result = pump_body(&mut client_read, &mut upstream_write, body_kind) => {
                match result {
                    Ok(bytes) => {
                        body_bytes = bytes;
                        body_ok = true;
                    }
                    Err(e) => debug!("Request body relay to {} ended: {}", target_addr, e),
                }
            }
            result = pump_response(&mut upstream_reader, &mut client_write, &head.method) => {
                outcome = Some(result?);
            }
        }

        let outcome = match outcome {
            Some(outcome) => outcome,
            None => pump_response(&mut upstream_reader, &mut client_write, &head.method).await?,
        };

        self.metrics.add_bytes(body_bytes, outcome.bytes);
        debug!(
            "Upstream {} answered {} with {} body bytes",
            target_addr, outcome.status, outcome.bytes
        );

        let reusable = pooling && body_ok && outcome.reusable;
        if reusable
            && upstream_reader.buffer().is_empty()
            && let Ok(stream) = upstream_reader.into_inner().reunite(upstream_write)
        {
            self.upstream.release(&target_addr, stream).await;
        }

        Ok(())
    }
}

/// Copies `len` bytes of request body, or a chunked body verbatim, upstream.
async fn pump_body<R, W>(
    client: &mut R,
    upstream: &mut W,
    body_kind: BodyKind,
) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let result = match body_kind {
        BodyKind::Empty => Ok(0),
        BodyKind::Length(len) => {
            let mut limited = client.take(len as u64);
            tokio::io::copy(&mut limited, upstream).await
        }
        BodyKind::Chunked => http1::copy_chunked(client, upstream).await,
    };

    match result {
        Ok(bytes) => {
            upstream.flush().await?;
            Ok(bytes)
        }
        Err(error) => {
            // The body will never be complete: tell the upstream so it can stop
            // waiting for more data and answer (or close) instead of stalling
            // the response side of the relay.
            let _ = upstream.shutdown().await;
            Err(error)
        }
    }
}

async fn pump_response<R, W>(
    reader: &mut R,
    client: &mut W,
    method: &str,
) -> std::io::Result<http1::RelayOutcome>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    http1::relay_response(reader, client, method).await
}

fn invalid_request(message: impl Into<String>) -> HttpProxyError {
    HttpProxyError::InvalidRequest(message.into())
}

fn build_request_head(head: &RequestHead, path: &str, keep_alive: bool) -> Vec<u8> {
    let mut data = Vec::with_capacity(512);
    data.extend_from_slice(
        format!("{} {} HTTP/1.{}\r\n", head.method, path, head.version_minor).as_bytes(),
    );

    let keep_transfer_encoding = head.is_chunked();

    for header in &head.headers {
        if header.lower.starts_with("proxy-") {
            continue;
        }

        if header.lower == "transfer-encoding" {
            if !keep_transfer_encoding {
                continue;
            }
        } else if is_hop_by_hop(&header.lower) {
            continue;
        }

        data.extend_from_slice(header.name.as_bytes());
        data.extend_from_slice(b": ");
        data.extend_from_slice(&header.value);
        data.extend_from_slice(b"\r\n");
    }

    data.extend_from_slice(if keep_alive {
        b"Connection: keep-alive\r\n\r\n"
    } else {
        b"Connection: close\r\n\r\n"
    });

    data
}

fn is_hop_by_hop(lower_name: &str) -> bool {
    matches!(
        lower_name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "upgrade"
            | "expect"
    )
}

fn connect_error_response(error: &forward::ConnectError) -> &'static [u8] {
    match error {
        forward::ConnectError::ConnectionTimeout => GATEWAY_TIMEOUT,
        _ => BAD_GATEWAY,
    }
}

/// Determines the upstream target from an absolute-URI or origin-form request.
fn resolve_target(head: &RequestHead) -> Result<(String, u16, String), HttpProxyError> {
    if let Ok(url) = url::Url::parse(&head.target)
        && let Some(host) = url.host_str()
    {
        let port = url.port_or_known_default().unwrap_or(80);
        let path = match url.query() {
            Some(query) => format!("{}?{}", url.path(), query),
            None => url.path().to_string(),
        };
        return Ok((host.to_string(), port, path));
    }

    // Origin-form: the client already speaks "proxy as gateway" style.
    if !head.target.starts_with('/') && head.target != "*" {
        return Err(invalid_request(format!(
            "Unsupported request target '{}'",
            head.target
        )));
    }

    let host_header = head
        .get("host")
        .ok_or_else(|| invalid_request("Missing Host header for origin-form request"))?;

    let (host, port) = parse_host_port(host_header, 80)?;
    Ok((host, port, head.target.clone()))
}

fn parse_host_port(value: &str, default_port: u16) -> Result<(String, u16), HttpProxyError> {
    let value = value.trim();

    if let Some(rest) = value.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| invalid_request(format!("Malformed Host header '{}'", value)))?;
        let host = rest[..end].to_string();
        let port = match rest[end + 1..].strip_prefix(':') {
            Some(port) if !port.is_empty() => port
                .parse::<u16>()
                .map_err(|_| invalid_request(format!("Invalid port in Host header '{}'", value)))?,
            _ => default_port,
        };
        return Ok((host, port));
    }

    // A bare IPv6 literal (invalid in a Host header but tolerated) has several colons.
    if value.matches(':').count() > 1 {
        return Ok((value.to_string(), default_port));
    }

    match value.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| invalid_request(format!("Invalid port in Host header '{}'", value)))?;
            Ok((host.to_string(), port))
        }
        Some(_) => Err(invalid_request(format!(
            "Invalid port in Host header '{}'",
            value
        ))),
        None => Ok((value.to_string(), default_port)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::options::ProxyOptions;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::oneshot;

    fn auth_manager(users: &[(&str, &str)]) -> Arc<AuthManager> {
        let map: HashMap<String, String> = users
            .iter()
            .map(|(u, p)| (u.to_string(), p.to_string()))
            .collect();
        Arc::new(AuthManager::new(&map).unwrap())
    }

    async fn start_proxy_with_metrics(
        users: &[(&str, &str)],
    ) -> (std::net::SocketAddr, Arc<Metrics>) {
        let auth_manager = auth_manager(users);
        let upstream = Arc::new(Upstream::new(ProxyOptions::for_tests()));
        let metrics = Metrics::new();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let proxy_metrics = metrics.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = BufferedConnection::new(stream, 4096);
            let proxy = HttpProxy::new(auth_manager, upstream, proxy_metrics);
            let _ = proxy.handle_connection(conn).await;
        });

        (addr, metrics)
    }

    async fn start_proxy(users: &[(&str, &str)]) -> std::net::SocketAddr {
        let (addr, _metrics) = start_proxy_with_metrics(users).await;
        addr
    }

    /// Serves `connections` clients through a single proxy instance (shared pool).
    async fn start_multi_proxy_with_metrics(
        connections: usize,
        users: &[(&str, &str)],
    ) -> (std::net::SocketAddr, Arc<Metrics>) {
        let auth_manager = auth_manager(users);
        let upstream = Arc::new(Upstream::new(ProxyOptions::for_tests()));
        let metrics = Metrics::new();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let proxy_metrics = metrics.clone();
        tokio::spawn(async move {
            for _ in 0..connections {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };

                let auth_manager = auth_manager.clone();
                let upstream = upstream.clone();
                let metrics = proxy_metrics.clone();

                tokio::spawn(async move {
                    let conn = BufferedConnection::new(stream, 4096);
                    let proxy = HttpProxy::new(auth_manager, upstream, metrics);
                    let _ = proxy.handle_connection(conn).await;
                });
            }
        });

        (addr, metrics)
    }

    async fn start_multi_proxy(connections: usize, users: &[(&str, &str)]) -> std::net::SocketAddr {
        let (addr, _metrics) = start_multi_proxy_with_metrics(connections, users).await;
        addr
    }

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

    /// Fake origin server that captures one request and replies with `response`.
    async fn start_origin_server(
        response: &'static [u8],
    ) -> (std::net::SocketAddr, oneshot::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_message(&mut stream).await;
            let _ = tx.send(request);
            stream.write_all(response).await.ok();
            stream.shutdown().await.ok();
        });

        (addr, rx)
    }

    /// Origin server that keeps the connection open and serves many requests.
    async fn start_keepalive_origin(
        response: &'static [u8],
    ) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = connections.clone();

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                counter.fetch_add(1, Ordering::SeqCst);

                tokio::spawn(async move {
                    loop {
                        let request = read_http_message(&mut stream).await;
                        if request.is_empty() {
                            break;
                        }
                        if stream.write_all(response).await.is_err() {
                            break;
                        }
                        if stream.flush().await.is_err() {
                            break;
                        }
                    }
                });
            }
        });

        (addr, connections)
    }

    /// Origin server that answers without reading the request body.
    async fn start_early_origin(response: &'static [u8]) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            let mut head = Vec::new();
            let mut chunk = [0u8; 512];
            loop {
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                head.extend_from_slice(&chunk[..n]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }

            stream.write_all(response).await.ok();
            stream.flush().await.ok();

            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        addr
    }

    async fn read_http_message(stream: &mut TcpStream) -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 512];

        loop {
            let n = stream.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..n]);

            if let Some(head_end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();

                if head
                    .to_ascii_lowercase()
                    .contains("transfer-encoding: chunked")
                {
                    if buffer[head_end + 4..].windows(5).any(|w| w == b"0\r\n\r\n") {
                        break;
                    }
                } else {
                    let content_length = head
                        .lines()
                        .filter_map(|line| {
                            let lower = line.to_ascii_lowercase();
                            lower
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .next()
                        .unwrap_or(0);

                    if buffer.len() >= head_end + 4 + content_length {
                        break;
                    }
                }
            }
        }

        buffer
    }

    #[tokio::test]
    async fn test_connect_tunnel() {
        let echo_addr = start_echo_server().await;
        let proxy_addr = start_proxy(&[]).await;

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n",
                    echo_addr, echo_addr
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let mut response = vec![0u8; CONNECT_OK.len()];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(response, CONNECT_OK);

        client.write_all(b"hello tunnel").await.unwrap();
        let mut echoed = [0u8; 12];
        client.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"hello tunnel");
    }

    #[tokio::test]
    async fn test_absolute_uri_request_forwards_origin_form() {
        let (origin_addr, request_rx) =
            start_origin_server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
        let proxy_addr = start_proxy(&[]).await;

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        let request = format!(
            "GET http://{}/index.html?q=1 HTTP/1.1\r\n\
             Host: {}\r\n\
             Proxy-Authorization: Basic Zm9vOmJhcg==\r\n\
             Proxy-Connection: keep-alive\r\n\
             X-Test: v\r\n\r\n",
            origin_addr, origin_addr
        );
        client.write_all(request.as_bytes()).await.unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("ok"));

        let forwarded = String::from_utf8_lossy(&request_rx.await.unwrap()).to_string();
        assert!(forwarded.starts_with("GET /index.html?q=1 HTTP/1.1\r\n"));
        assert!(forwarded.contains("X-Test: v"));
        assert!(
            !forwarded
                .to_ascii_lowercase()
                .contains("proxy-authorization")
        );
        assert!(!forwarded.to_ascii_lowercase().contains("proxy-connection"));
        assert!(forwarded.contains("Host:"));
    }

    #[tokio::test]
    async fn test_origin_form_with_host_header() {
        let (origin_addr, request_rx) =
            start_origin_server(b"HTTP/1.1 204 No Content\r\n\r\n").await;
        let proxy_addr = start_proxy(&[]).await;

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(format!("HEAD /health HTTP/1.1\r\nHost: {}\r\n\r\n", origin_addr).as_bytes())
            .await
            .unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();

        let forwarded = String::from_utf8_lossy(&request_rx.await.unwrap()).to_string();
        assert!(forwarded.starts_with("HEAD /health HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn test_chunked_body_is_passed_through() {
        let (origin_addr, request_rx) =
            start_origin_server(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        let proxy_addr = start_proxy(&[]).await;

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        let request = format!(
            "POST http://{}/upload HTTP/1.1\r\n\
             Host: {}\r\n\
             Transfer-Encoding: chunked\r\n\r\n\
             5\r\nhello\r\n\
             6\r\n world\r\n\
             0\r\n\r\n",
            origin_addr, origin_addr
        );
        client.write_all(request.as_bytes()).await.unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();

        let forwarded = String::from_utf8_lossy(&request_rx.await.unwrap()).to_string();
        assert!(
            forwarded
                .to_ascii_lowercase()
                .contains("transfer-encoding: chunked")
        );
        assert!(forwarded.contains("5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"));
    }

    #[tokio::test]
    async fn test_content_length_body_is_streamed() {
        let (origin_addr, request_rx) =
            start_origin_server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
        let proxy_addr = start_proxy(&[]).await;

        let body = vec![b'x'; 256 * 1024];
        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "POST http://{}/upload HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\n\r\n",
                    origin_addr,
                    origin_addr,
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        client.write_all(&body).await.unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("200 OK"));

        let forwarded = request_rx.await.unwrap();
        assert!(forwarded.ends_with(&body));
    }

    /// A server that answers before consuming the body must not deadlock the proxy.
    #[tokio::test]
    async fn test_early_response_is_relayed() {
        let origin_addr =
            start_early_origin(b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\n\r\n")
                .await;
        let proxy_addr = start_proxy(&[]).await;

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "POST http://{}/upload HTTP/1.1\r\nHost: {}\r\nContent-Length: 1048576\r\n\r\n",
                    origin_addr, origin_addr
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let mut response = Vec::new();
        let mut chunk = [0u8; 256];

        while !String::from_utf8_lossy(&response).contains("413") {
            let read = timeout(Duration::from_secs(5), client.read(&mut chunk))
                .await
                .expect("proxy deadlocked waiting for the request body")
                .unwrap();

            if read == 0 {
                break;
            }

            response.extend_from_slice(&chunk[..read]);
        }

        assert!(String::from_utf8_lossy(&response).contains("413"));
    }

    #[tokio::test]
    async fn test_ambiguous_framing_is_rejected() {
        let proxy_addr = start_proxy(&[]).await;
        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                b"POST http://example.com/ HTTP/1.1\r\n\
                  Host: example.com\r\n\
                  Content-Length: 5\r\n\
                  Transfer-Encoding: chunked\r\n\r\n",
            )
            .await
            .unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("400"));
    }

    #[tokio::test]
    async fn test_auth_required_and_success() {
        let (origin_addr, _request_rx) =
            start_origin_server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;

        let (proxy_addr, metrics) = start_proxy_with_metrics(&[("alice", "secret")]).await;
        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "GET http://{}/ HTTP/1.1\r\nHost: {}\r\n\r\n",
                    origin_addr, origin_addr
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("407"));
        assert!(
            metrics
                .render()
                .contains("rust_proxy_auth_failures_total 1")
        );

        let proxy_addr = start_proxy(&[("alice", "secret")]).await;
        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        let credentials = general_purpose::STANDARD.encode("alice:secret");
        client
            .write_all(
                format!(
                    "GET http://{}/ HTTP/1.1\r\nHost: {}\r\nProxy-Authorization: Basic {}\r\n\r\n",
                    origin_addr, origin_addr, credentials
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("200 OK"));
    }

    /// Missing and wrong credentials must each be counted exactly once.
    #[tokio::test]
    async fn test_auth_failures_are_counted_once_each() {
        let (origin_addr, _request_rx) =
            start_origin_server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
        let (proxy_addr, metrics) = start_multi_proxy_with_metrics(3, &[("alice", "secret")]).await;

        let cases = [
            None,
            Some(general_purpose::STANDARD.encode("alice:wrong")),
            Some(general_purpose::STANDARD.encode("alice:secret")),
        ];

        for encoded in cases {
            let mut client = TcpStream::connect(proxy_addr).await.unwrap();
            let mut request =
                format!("GET http://{origin_addr}/ HTTP/1.1\r\nHost: {origin_addr}\r\n");

            if let Some(encoded) = encoded {
                request.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
            }
            request.push_str("\r\n");

            client.write_all(request.as_bytes()).await.unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
        }

        let rendered = metrics.render();
        assert!(
            rendered.contains("rust_proxy_auth_failures_total 2"),
            "{rendered}"
        );
    }

    #[tokio::test]
    async fn test_upstream_connection_is_reused() {
        let (origin_addr, connections) =
            start_keepalive_origin(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
        let proxy_addr = start_multi_proxy(3, &[]).await;

        for _ in 0..3 {
            let mut client = TcpStream::connect(proxy_addr).await.unwrap();
            client
                .write_all(
                    format!(
                        "GET http://{}/ HTTP/1.1\r\nHost: {}\r\n\r\n",
                        origin_addr, origin_addr
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();

            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            assert!(String::from_utf8_lossy(&response).contains("200 OK"));
        }

        assert_eq!(
            connections.load(Ordering::SeqCst),
            1,
            "upstream connection should be pooled and reused"
        );
    }

    #[tokio::test]
    async fn test_response_connection_header_is_rewritten() {
        let (origin_addr, _request_rx) = start_origin_server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
        )
        .await;
        let proxy_addr = start_proxy(&[]).await;

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(
                format!(
                    "GET http://{}/ HTTP/1.1\r\nHost: {}\r\n\r\n",
                    origin_addr, origin_addr
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let text = String::from_utf8_lossy(&response).to_ascii_lowercase();

        assert!(text.contains("connection: close"));
        assert!(!text.contains("connection: keep-alive"));
    }

    #[tokio::test]
    async fn test_unsupported_method_returns_501() {
        let proxy_addr = start_proxy(&[]).await;
        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client
            .write_all(b"TRACE http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("501"));
    }

    #[tokio::test]
    async fn test_parse_host_port() {
        assert_eq!(
            parse_host_port("example.com:8080", 80).unwrap(),
            ("example.com".to_string(), 8080)
        );
        assert_eq!(
            parse_host_port("example.com", 80).unwrap(),
            ("example.com".to_string(), 80)
        );
        assert_eq!(
            parse_host_port("[::1]:443", 80).unwrap(),
            ("::1".to_string(), 443)
        );
        assert_eq!(
            parse_host_port("[::1]", 80).unwrap(),
            ("::1".to_string(), 80)
        );
        assert!(parse_host_port("example.com:abc", 80).is_err());
    }

    #[tokio::test]
    async fn test_resolve_target_rejects_garbage() {
        let head = RequestHead {
            method: "GET".to_string(),
            target: "not-a-valid-target".to_string(),
            version_minor: 1,
            headers: Vec::new(),
        };
        assert!(resolve_target(&head).is_err());
    }
}
