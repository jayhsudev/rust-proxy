use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::net::conn::{BufferedConnection, tune_stream};
use crate::proxy::options::ProxyOptions;
use crate::proxy::pool::ConnectionPool;

/// Upper bound for cached DNS answers.
const MAX_DNS_ENTRIES: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("Failed to resolve address: {0}")]
    AddressResolutionFailed(String),
    #[error("Connection timed out")]
    ConnectionTimeout,
    #[error("Connection refused: {0}")]
    ConnectionRefused(String),
    #[error("Target address not found")]
    AddressNotFound,
}

/// Joins a host and a port into an address string, bracketing IPv6 literals.
pub fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]:{}", host, port)
    } else {
        format!("{}:{}", host, port)
    }
}

struct DnsEntry {
    addresses: Vec<SocketAddr>,
    expires_at: Instant,
}

/// Resolves, connects and pools upstream connections.
pub struct Upstream {
    options: ProxyOptions,
    dns_cache: Mutex<HashMap<String, DnsEntry>>,
    pool: ConnectionPool,
}

impl Upstream {
    pub fn new(options: ProxyOptions) -> Self {
        let pool = ConnectionPool::new(
            options.upstream_pool_size,
            options.upstream_pool_idle_timeout,
        );

        Self {
            options,
            dns_cache: Mutex::new(HashMap::new()),
            pool,
        }
    }

    pub fn options(&self) -> &ProxyOptions {
        &self.options
    }

    /// Whether upstream connections may be reused for HTTP keep-alive.
    pub fn pooling_enabled(&self) -> bool {
        self.options.upstream_keepalive && self.pool.enabled()
    }

    /// Resolves and connects to `addr` (host:port), sharing a single
    /// `connect_timeout` budget across every resolved address.
    pub async fn connect(&self, addr: &str) -> Result<TcpStream, ConnectError> {
        let addresses = self.resolve(addr).await?;
        let deadline = Instant::now() + self.options.connect_timeout;
        let mut last_error = ConnectError::AddressNotFound;

        for address in addresses {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ConnectError::ConnectionTimeout);
            }

            match timeout(remaining, TcpStream::connect(address)).await {
                Ok(Ok(stream)) => {
                    tune_stream(&stream, self.options.keepalive)?;
                    return Ok(stream);
                }
                Ok(Err(e)) => last_error = ConnectError::ConnectionRefused(e.to_string()),
                Err(_) => last_error = ConnectError::ConnectionTimeout,
            }
        }

        Err(last_error)
    }

    /// Takes a reusable upstream connection, if one is idle and still healthy.
    pub async fn take_pooled(&self, addr: &str) -> Option<TcpStream> {
        self.pool.take(addr).await
    }

    /// Returns a healthy upstream connection to the pool.
    pub async fn release(&self, addr: &str, stream: TcpStream) {
        self.pool.put(addr, stream).await;
    }

    async fn resolve(&self, addr: &str) -> Result<Vec<SocketAddr>, ConnectError> {
        if self.options.dns_cache_ttl.is_some() {
            let cache = self.dns_cache.lock().await;
            if let Some(entry) = cache.get(addr)
                && entry.expires_at > Instant::now()
            {
                return Ok(entry.addresses.clone());
            }
        }

        let resolved = timeout(self.options.connect_timeout, tokio::net::lookup_host(addr))
            .await
            .map_err(|_| ConnectError::ConnectionTimeout)?
            .map_err(|e| ConnectError::AddressResolutionFailed(e.to_string()))?;

        let addresses: Vec<SocketAddr> = resolved.collect();
        if addresses.is_empty() {
            return Err(ConnectError::AddressNotFound);
        }

        if let Some(ttl) = self.options.dns_cache_ttl {
            let mut cache = self.dns_cache.lock().await;

            if cache.len() >= MAX_DNS_ENTRIES {
                let now = Instant::now();
                cache.retain(|_, entry| entry.expires_at > now);

                if cache.len() >= MAX_DNS_ENTRIES {
                    cache.clear();
                }
            }

            cache.insert(
                addr.to_string(),
                DnsEntry {
                    addresses: addresses.clone(),
                    expires_at: Instant::now() + ttl,
                },
            );
        }

        Ok(addresses)
    }

    #[cfg(test)]
    pub(crate) async fn dns_cache_entries(&self) -> usize {
        self.dns_cache.lock().await.len()
    }

    #[cfg(test)]
    pub(crate) async fn pooled_entries(&self) -> usize {
        self.pool.idle_count().await
    }
}

/// Relays traffic in both directions with a configurable buffer size.
///
/// Half-close is propagated: when one side reaches EOF its write half is shut
/// down while the opposite direction keeps flowing, so protocols that rely on
/// "request finished, response still streaming" work correctly.
pub async fn relay(
    client: BufferedConnection,
    target: BufferedConnection,
    buffer_size: usize,
) -> io::Result<(u64, u64)> {
    let buffer_size = buffer_size.max(1024);
    let (mut client_read, mut client_write) = client.into_split();
    let (mut target_read, mut target_write) = target.into_split();

    let (client_to_target, target_to_client) = tokio::try_join!(
        copy_one_way(&mut client_read, &mut target_write, buffer_size),
        copy_one_way(&mut target_read, &mut client_write, buffer_size),
    )?;

    log::debug!(
        "Relayed {} bytes client->target, {} bytes target->client",
        client_to_target,
        target_to_client
    );

    Ok((client_to_target, target_to_client))
}

async fn copy_one_way<R, W>(reader: &mut R, writer: &mut W, buffer_size: usize) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; buffer_size];
    let mut total = 0u64;

    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };

        if read == 0 {
            break;
        }

        writer.write_all(&buffer[..read]).await?;
        total += read as u64;
    }

    writer.flush().await?;
    writer.shutdown().await?;

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::TcpListener;

    fn test_options() -> ProxyOptions {
        ProxyOptions::for_tests()
    }

    #[test]
    fn test_join_host_port() {
        assert_eq!(join_host_port("example.com", 80), "example.com:80");
        assert_eq!(join_host_port("127.0.0.1", 8080), "127.0.0.1:8080");
        assert_eq!(join_host_port("::1", 1080), "[::1]:1080");
        assert_eq!(join_host_port("[::1]", 1080), "[::1]:1080");
    }

    #[tokio::test]
    async fn test_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await });

        let upstream = Upstream::new(test_options());
        let stream = upstream.connect(&addr.to_string()).await.unwrap();
        assert!(stream.peer_addr().is_ok());
        assert!(accept.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn test_connect_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let upstream = Upstream::new(test_options());
        let err = upstream.connect(&addr.to_string()).await.unwrap_err();
        assert!(matches!(
            err,
            ConnectError::ConnectionRefused(_) | ConnectError::ConnectionTimeout
        ));
    }

    #[tokio::test]
    async fn test_unresolvable_address() {
        let upstream = Upstream::new(test_options());
        let err = upstream
            .connect("this-host-does-not-exist.invalid:80")
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ConnectError::AddressResolutionFailed(_) | ConnectError::ConnectionTimeout
            ),
            "unexpected error for an unresolvable target: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_dns_cache() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let mut options = test_options();
        options.dns_cache_ttl = Some(Duration::from_secs(30));
        let upstream = Upstream::new(options);

        assert_eq!(upstream.dns_cache_entries().await, 0);
        upstream.connect(&addr.to_string()).await.unwrap();
        assert_eq!(upstream.dns_cache_entries().await, 1);
        upstream.connect(&addr.to_string()).await.unwrap();
        assert_eq!(upstream.dns_cache_entries().await, 1);
    }

    #[tokio::test]
    async fn test_pool_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let key = addr.to_string();

        let upstream = Upstream::new(test_options());
        assert!(upstream.pooling_enabled());
        assert!(upstream.take_pooled(&key).await.is_none());

        let stream = upstream.connect(&key).await.unwrap();
        upstream.release(&key, stream).await;
        assert_eq!(upstream.pooled_entries().await, 1);

        assert!(upstream.take_pooled(&key).await.is_some());
        assert_eq!(upstream.pooled_entries().await, 0);
    }

    /// A client half-close must be forwarded to the target, and the response
    /// direction must keep flowing afterwards.
    #[tokio::test]
    async fn test_relay_propagates_half_close() {
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = echo.accept().await.unwrap();
            let (mut reader, mut writer) = stream.split();
            tokio::io::copy(&mut reader, &mut writer).await.unwrap();
            writer.shutdown().await.unwrap();
        });

        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        tokio::spawn(async move {
            let (client, _) = proxy.accept().await.unwrap();
            let target = TcpStream::connect(echo_addr).await.unwrap();
            let result = relay(
                BufferedConnection::new(client, 4096),
                BufferedConnection::new(target, 4096),
                4096,
            )
            .await;
            assert!(result.is_ok(), "relay failed: {:?}", result.err());
        });

        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client.write_all(b"hello relay").await.unwrap();
        client.shutdown().await.unwrap();

        let mut echoed = Vec::new();
        client.read_to_end(&mut echoed).await.unwrap();
        assert_eq!(echoed, b"hello relay");
    }
}
