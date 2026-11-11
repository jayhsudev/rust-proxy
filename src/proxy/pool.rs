use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// Upper bound for distinct upstream hosts tracked at once.
const MAX_HOSTS: usize = 1024;

struct Pooled {
    stream: TcpStream,
    idle_since: Instant,
}

/// Idle upstream connections, keyed by `host:port`, reused across client
/// connections to avoid a DNS lookup plus TCP handshake per request.
pub struct ConnectionPool {
    idle: Mutex<HashMap<String, Vec<Pooled>>>,
    max_per_host: usize,
    idle_timeout: Duration,
}

impl ConnectionPool {
    pub fn new(max_per_host: usize, idle_timeout: Duration) -> Self {
        Self {
            idle: Mutex::new(HashMap::new()),
            max_per_host,
            idle_timeout,
        }
    }

    pub fn enabled(&self) -> bool {
        self.max_per_host > 0 && !self.idle_timeout.is_zero()
    }

    /// Takes a live idle connection, discarding stale or half-closed ones.
    pub async fn take(&self, key: &str) -> Option<TcpStream> {
        if !self.enabled() {
            return None;
        }

        let mut map = self.idle.lock().await;
        let entries = map.get_mut(key)?;

        while let Some(pooled) = entries.pop() {
            if pooled.idle_since.elapsed() >= self.idle_timeout {
                continue;
            }

            if is_reusable(&pooled.stream) {
                if entries.is_empty() {
                    map.remove(key);
                }
                return Some(pooled.stream);
            }
        }

        map.remove(key);
        None
    }

    /// Returns a healthy connection to the pool (dropped when the pool is full).
    pub async fn put(&self, key: &str, stream: TcpStream) {
        if !self.enabled() {
            return;
        }

        let mut map = self.idle.lock().await;

        if map.len() >= MAX_HOSTS && !map.contains_key(key) {
            let now = Instant::now();
            map.retain(|_, entries| {
                entries.retain(|pooled| now.duration_since(pooled.idle_since) < self.idle_timeout);
                !entries.is_empty()
            });

            if map.len() >= MAX_HOSTS {
                map.clear();
            }
        }

        let entries = map.entry(key.to_string()).or_default();

        if entries.len() >= self.max_per_host {
            entries.remove(0);
        }

        entries.push(Pooled {
            stream,
            idle_since: Instant::now(),
        });
    }

    #[cfg(test)]
    pub(crate) async fn idle_count(&self) -> usize {
        self.idle.lock().await.values().map(Vec::len).sum()
    }
}

/// A pooled connection is only reusable when it is readable-but-idle: pending
/// data or EOF means the server side changed state behind our back.
fn is_reusable(stream: &TcpStream) -> bool {
    let mut probe = [0u8; 1];

    match stream.try_read(&mut probe) {
        Ok(0) => false,
        Ok(_) => false,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    async fn connected_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (client, server)
    }

    #[tokio::test]
    async fn test_put_and_take() {
        let pool = ConnectionPool::new(4, Duration::from_secs(30));
        let (client, _server) = connected_pair().await;

        assert_eq!(pool.idle_count().await, 0);
        pool.put("example.com:80", client).await;
        assert_eq!(pool.idle_count().await, 1);

        let reused = pool.take("example.com:80").await;
        assert!(reused.is_some());
        assert_eq!(pool.idle_count().await, 0);
    }

    #[tokio::test]
    async fn test_expired_connection_is_dropped() {
        let pool = ConnectionPool::new(4, Duration::from_millis(1));
        let (client, _server) = connected_pair().await;

        pool.put("example.com:80", client).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(pool.take("example.com:80").await.is_none());
        assert_eq!(pool.idle_count().await, 0);
    }

    #[tokio::test]
    async fn test_closed_connection_is_not_reused() {
        let pool = ConnectionPool::new(4, Duration::from_secs(30));
        let (client, mut server) = connected_pair().await;

        pool.put("example.com:80", client).await;
        server.shutdown().await.unwrap();
        drop(server);

        // Give the FIN time to arrive.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(pool.take("example.com:80").await.is_none());
    }

    #[tokio::test]
    async fn test_pool_is_bounded_per_host() {
        let pool = ConnectionPool::new(2, Duration::from_secs(30));

        for _ in 0..5 {
            let (client, _server) = connected_pair().await;
            pool.put("example.com:80", client).await;
        }

        assert_eq!(pool.idle_count().await, 2);
    }

    #[tokio::test]
    async fn test_disabled_pool_ignores_connections() {
        let pool = ConnectionPool::new(0, Duration::from_secs(30));
        let (client, _server) = connected_pair().await;

        assert!(!pool.enabled());
        pool.put("example.com:80", client).await;
        assert_eq!(pool.idle_count().await, 0);
        assert!(pool.take("example.com:80").await.is_none());
    }
}
