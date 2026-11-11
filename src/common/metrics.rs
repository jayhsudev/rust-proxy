use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_REQUEST_BYTES: usize = 8 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Runtime counters exposed over the optional metrics endpoint.
#[derive(Default)]
pub struct Metrics {
    connections_total: AtomicU64,
    connections_active: AtomicI64,
    connections_rejected_limit: AtomicU64,
    connections_rejected_ip: AtomicU64,
    auth_failures_total: AtomicU64,
    upstream_errors_total: AtomicU64,
    http_requests_total: AtomicU64,
    socks5_connects_total: AtomicU64,
    pool_hits_total: AtomicU64,
    pool_misses_total: AtomicU64,
    bytes_to_target_total: AtomicU64,
    bytes_to_client_total: AtomicU64,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn connection_opened(&self) {
        self.connections_total.fetch_add(1, Ordering::Relaxed);
        self.connections_active.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_closed(&self) {
        self.connections_active.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn rejected_limit(&self) {
        self.connections_rejected_limit
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn rejected_ip(&self) {
        self.connections_rejected_ip.fetch_add(1, Ordering::Relaxed);
    }

    pub fn auth_failure(&self) {
        self.auth_failures_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn upstream_error(&self) {
        self.upstream_errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn http_request(&self) {
        self.http_requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn socks5_connect(&self) {
        self.socks5_connects_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn pool_hit(&self) {
        self.pool_hits_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn pool_miss(&self) {
        self.pool_misses_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_bytes(&self, to_target: u64, to_client: u64) {
        self.bytes_to_target_total
            .fetch_add(to_target, Ordering::Relaxed);
        self.bytes_to_client_total
            .fetch_add(to_client, Ordering::Relaxed);
    }

    /// Prometheus text exposition format.
    pub fn render(&self) -> String {
        let counters = [
            (
                "rust_proxy_connections_total",
                "counter",
                "Accepted client connections",
                self.connections_total.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_connections_active",
                "gauge",
                "Currently active client connections",
                self.connections_active.load(Ordering::Relaxed),
            ),
            (
                "rust_proxy_connections_rejected_limit_total",
                "counter",
                "Connections rejected by the global limit",
                self.connections_rejected_limit.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_connections_rejected_ip_total",
                "counter",
                "Connections rejected by the per-IP limit",
                self.connections_rejected_ip.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_auth_failures_total",
                "counter",
                "Failed authentication attempts",
                self.auth_failures_total.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_upstream_errors_total",
                "counter",
                "Upstream connect / relay failures",
                self.upstream_errors_total.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_http_requests_total",
                "counter",
                "Forwarded HTTP requests",
                self.http_requests_total.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_socks5_connects_total",
                "counter",
                "Established SOCKS5 CONNECT tunnels",
                self.socks5_connects_total.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_pool_hits_total",
                "counter",
                "Upstream connections reused from the pool",
                self.pool_hits_total.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_pool_misses_total",
                "counter",
                "Requests that required a new upstream connection",
                self.pool_misses_total.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_bytes_to_target_total",
                "counter",
                "Bytes relayed from clients to targets",
                self.bytes_to_target_total.load(Ordering::Relaxed) as i64,
            ),
            (
                "rust_proxy_bytes_to_client_total",
                "counter",
                "Bytes relayed from targets to clients",
                self.bytes_to_client_total.load(Ordering::Relaxed) as i64,
            ),
        ];

        let mut out = String::with_capacity(1024);
        for (name, kind, help, value) in counters {
            out.push_str(&format!("# HELP {name} {help}\n"));
            out.push_str(&format!("# TYPE {name} {kind}\n"));
            out.push_str(&format!("{name} {value}\n"));
        }
        out
    }
}

/// Serves `/metrics` (Prometheus) and `/healthz` on the given listener.
pub async fn serve(listener: TcpListener, metrics: Arc<Metrics>) {
    loop {
        let Ok((stream, _peer)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };

        let metrics = metrics.clone();
        tokio::spawn(async move {
            handle_connection(stream, metrics).await;
        });
    }
}

async fn handle_connection(mut stream: TcpStream, metrics: Arc<Metrics>) {
    let mut buffer = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];

    let read_request = async {
        loop {
            if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                return true;
            }
            if buffer.len() > MAX_REQUEST_BYTES {
                return false;
            }

            match stream.read(&mut chunk).await {
                Ok(0) => return false,
                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                Err(_) => return false,
            }
        }
    };

    if !matches!(
        tokio::time::timeout(REQUEST_TIMEOUT, read_request).await,
        Ok(true)
    ) {
        return;
    }

    let request = String::from_utf8_lossy(&buffer);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let (status, body, content_type) = route(path, &metrics);
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn route(path: &str, metrics: &Metrics) -> (&'static str, String, &'static str) {
    match path.split('?').next().unwrap_or("/") {
        "/metrics" => (
            "200 OK",
            metrics.render(),
            "text/plain; version=0.0.4; charset=utf-8",
        ),
        "/healthz" => ("200 OK", "ok\n".to_string(), "text/plain; charset=utf-8"),
        "/" => (
            "200 OK",
            "rust-proxy\nendpoints: /metrics /healthz\n".to_string(),
            "text/plain; charset=utf-8",
        ),
        _ => (
            "404 Not Found",
            "not found\n".to_string(),
            "text/plain; charset=utf-8",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counters_and_render() {
        let metrics = Metrics::new();
        metrics.connection_opened();
        metrics.connection_opened();
        metrics.connection_closed();
        metrics.rejected_ip();
        metrics.add_bytes(10, 20);
        metrics.pool_hit();

        let text = metrics.render();
        assert!(text.contains("rust_proxy_connections_total 2"));
        assert!(text.contains("rust_proxy_connections_active 1"));
        assert!(text.contains("rust_proxy_connections_rejected_ip_total 1"));
        assert!(text.contains("# TYPE rust_proxy_bytes_to_target_total counter"));
        assert!(text.contains("rust_proxy_bytes_to_target_total 10"));
        assert!(text.contains("rust_proxy_bytes_to_client_total 20"));
        assert!(text.contains("rust_proxy_pool_hits_total 1"));
    }

    #[test]
    fn test_routes() {
        let metrics = Metrics::new();

        let (status, body, _) = route("/metrics", &metrics);
        assert_eq!(status, "200 OK");
        assert!(body.contains("rust_proxy_connections_total"));

        let (status, body, _) = route("/healthz", &metrics);
        assert_eq!(status, "200 OK");
        assert_eq!(body, "ok\n");

        let (status, _, _) = route("/metrics?x=1", &metrics);
        assert_eq!(status, "200 OK");

        let (status, _, _) = route("/nope", &metrics);
        assert_eq!(status, "404 Not Found");
    }

    #[tokio::test]
    async fn test_serve_healthz() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let metrics = Metrics::new();

        tokio::spawn(async move {
            serve(listener, metrics).await;
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let text = String::from_utf8_lossy(&response);

        assert!(text.starts_with("HTTP/1.1 200 OK"));
        assert!(text.ends_with("ok\n"));
    }
}
