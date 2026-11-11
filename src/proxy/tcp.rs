use log::{debug, info, warn};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task;
use tokio::time::timeout;

use crate::common::auth::AuthManager;
use crate::common::metrics::Metrics;
use crate::net::conn::{BufferedConnection, tune_stream};
use crate::proxy::forward::Upstream;
use crate::proxy::http::HttpProxy;
use crate::proxy::options::ProxyOptions;
use crate::proxy::socks5::Socks5Proxy;

/// Grace period for in-flight connections when shutting down.
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(10);
/// Base delay for the accept-loop backoff.
const ACCEPT_BACKOFF_BASE: Duration = Duration::from_millis(50);
/// Upper bound for the accept-loop backoff.
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_millis(3200);

#[derive(Error, Debug)]
pub enum TcpProxyError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("No data received from client")]
    NoDataReceived,
    #[error("Client handshake timed out")]
    HandshakeTimeout,
    #[error("Unsupported protocol (first byte: {0:#04x})")]
    UnsupportedProtocol(u8),
    #[error("HTTP proxy error: {0}")]
    HttpProxyError(#[from] crate::proxy::http::HttpProxyError),
    #[error("SOCKS5 proxy error: {0}")]
    Socks5ProxyError(#[from] crate::proxy::socks5::Socks5ProxyError),
}

/// Per-IP concurrency limiter. A limit of 0 disables tracking entirely.
struct IpLimiter {
    limit: usize,
    counts: Mutex<HashMap<IpAddr, usize>>,
}

struct IpGuard {
    ip: IpAddr,
    limiter: Arc<IpLimiter>,
    tracked: bool,
}

impl IpLimiter {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            counts: Mutex::new(HashMap::new()),
        })
    }

    fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<IpGuard> {
        if self.limit == 0 {
            return Some(IpGuard {
                ip,
                limiter: self.clone(),
                tracked: false,
            });
        }

        let mut counts = self.lock();
        let count = counts.entry(ip).or_insert(0);

        if *count >= self.limit {
            return None;
        }

        *count += 1;
        drop(counts);

        Some(IpGuard {
            ip,
            limiter: self.clone(),
            tracked: true,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<IpAddr, usize>> {
        self.counts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for IpGuard {
    fn drop(&mut self) {
        if !self.tracked {
            return;
        }

        let mut counts = self.limiter.lock();
        if let Some(count) = counts.get_mut(&self.ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(&self.ip);
            }
        }
    }
}

pub struct TcpProxy {
    auth_manager: Arc<AuthManager>,
    upstream: Arc<Upstream>,
    options: Arc<ProxyOptions>,
    metrics: Arc<Metrics>,
    semaphore: Arc<Semaphore>,
    max_connections: usize,
    ip_limiter: Arc<IpLimiter>,
}

impl TcpProxy {
    pub fn new(
        auth_manager: Arc<AuthManager>,
        upstream: Arc<Upstream>,
        options: Arc<ProxyOptions>,
        metrics: Arc<Metrics>,
        max_connections: usize,
        max_connections_per_ip: usize,
    ) -> Self {
        TcpProxy {
            auth_manager,
            upstream,
            options,
            metrics,
            semaphore: Arc::new(Semaphore::new(max_connections)),
            max_connections,
            ip_limiter: IpLimiter::new(max_connections_per_ip),
        }
    }

    /// Accept connections until Ctrl-C / SIGTERM is received.
    pub async fn run(&self, listener: TcpListener) {
        info!(
            "TCP proxy listening on {}",
            listener
                .local_addr()
                .map(|addr| addr.to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        );

        let mut shutdown = std::pin::pin!(shutdown_signal());
        let mut accept_errors: u32 = 0;
        let rejected_total = AtomicU64::new(0);
        let rejected_ip = AtomicU64::new(0);

        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    info!("Received shutdown signal");
                    break;
                }
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            accept_errors = 0;

                            let Some(ip_guard) = self.ip_limiter.try_acquire(addr.ip()) else {
                                self.metrics.rejected_ip();
                                if should_log(&rejected_ip) {
                                    warn!(
                                        "Per-IP connection limit ({}) reached, rejecting {}",
                                        self.ip_limiter.limit, addr
                                    );
                                }
                                drop(stream);
                                continue;
                            };

                            let permit = match self.semaphore.clone().try_acquire_owned() {
                                Ok(permit) => permit,
                                Err(_) => {
                                    self.metrics.rejected_limit();
                                    if should_log(&rejected_total) {
                                        warn!(
                                            "Max connections ({}) reached, rejecting {}",
                                            self.max_connections, addr
                                        );
                                    }
                                    drop(stream);
                                    continue;
                                }
                            };

                            let auth_manager = self.auth_manager.clone();
                            let upstream = self.upstream.clone();
                            let options = self.options.clone();
                            let metrics = self.metrics.clone();

                            self.metrics.connection_opened();

                            task::spawn(async move {
                                let result = Self::handle_connection(
                                    stream,
                                    addr,
                                    auth_manager,
                                    upstream,
                                    options,
                                    metrics.clone(),
                                )
                                .await;

                                if let Err(e) = result {
                                    log_connection_error(addr, &e);
                                }

                                metrics.connection_closed();
                                drop(ip_guard);
                                drop(permit);
                            });
                        }
                        Err(e) => {
                            // Resource exhaustion (EMFILE/ENFILE) repeats immediately:
                            // back off exponentially and keep the log quiet.
                            accept_errors = accept_errors.saturating_add(1);

                            if accept_errors == 1 || accept_errors.is_multiple_of(100) {
                                warn!("Accept error ({} in a row): {}", accept_errors, e);
                            }

                            let factor = 1u32 << accept_errors.min(6);
                            let backoff = (ACCEPT_BACKOFF_BASE * factor).min(ACCEPT_BACKOFF_MAX);
                            tokio::time::sleep(backoff).await;
                        }
                    }
                }
            }
        }

        info!("Stopped accepting new connections, waiting for in-flight connections");

        // All connection permits are returned once the corresponding task finishes.
        let all_permits = u32::try_from(self.max_connections).unwrap_or(u32::MAX);
        if timeout(
            SHUTDOWN_GRACE_PERIOD,
            self.semaphore.acquire_many(all_permits),
        )
        .await
        .is_err()
        {
            warn!("Shutdown grace period elapsed, aborting remaining connections");
        } else {
            info!("All connections closed");
        }

        info!("Proxy stopped");
    }

    async fn handle_connection(
        stream: TcpStream,
        addr: std::net::SocketAddr,
        auth_manager: Arc<AuthManager>,
        upstream: Arc<Upstream>,
        options: Arc<ProxyOptions>,
        metrics: Arc<Metrics>,
    ) -> Result<(), TcpProxyError> {
        tune_stream(&stream, options.keepalive)?;

        let mut conn = BufferedConnection::new(stream, options.buffer_size);

        let bytes_read = timeout(options.handshake_timeout, conn.read())
            .await
            .map_err(|_| TcpProxyError::HandshakeTimeout)??;

        if bytes_read == 0 {
            return Err(TcpProxyError::NoDataReceived);
        }

        let first_byte = conn
            .read_from_buffer(1)
            .map(|byte| byte[0])
            .ok_or(TcpProxyError::NoDataReceived)?;
        conn.unread(&[first_byte]);

        match first_byte {
            // SOCKS5 protocol starts with 0x05
            0x05 => {
                debug!("SOCKS5 connection from {}", addr);
                Socks5Proxy::new(auth_manager, upstream, metrics)
                    .handle_connection(conn)
                    .await?;
            }
            // HTTP methods start with ASCII letters
            b'A'..=b'Z' | b'a'..=b'z' => {
                debug!("HTTP connection from {}", addr);
                HttpProxy::new(auth_manager, upstream, metrics)
                    .handle_connection(conn)
                    .await?;
            }
            other => {
                return Err(TcpProxyError::UnsupportedProtocol(other));
            }
        }

        Ok(())
    }
}

/// Rate-limits repeated warnings: first one plus every 100th.
fn should_log(counter: &AtomicU64) -> bool {
    let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
    count == 1 || count.is_multiple_of(100)
}

/// Client-side disconnects and protocol probing are expected, only log them at debug level.
fn log_connection_error(addr: std::net::SocketAddr, error: &TcpProxyError) {
    let expected = match error {
        TcpProxyError::NoDataReceived
        | TcpProxyError::HandshakeTimeout
        | TcpProxyError::UnsupportedProtocol(_) => true,
        TcpProxyError::IoError(e) => is_expected_io_error(e),
        TcpProxyError::HttpProxyError(http_error) => match http_error {
            crate::proxy::http::HttpProxyError::ProxyAuthRequired => true,
            crate::proxy::http::HttpProxyError::IoError(e) => is_expected_io_error(e),
            _ => false,
        },
        TcpProxyError::Socks5ProxyError(socks_error) => match socks_error {
            crate::proxy::socks5::Socks5ProxyError::IoError(e) => is_expected_io_error(e),
            _ => false,
        },
    };

    if expected {
        debug!("Connection from {} closed: {}", addr, error);
    } else {
        warn!("Connection error from {}: {}", addr, error);
    }
}

fn is_expected_io_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::UnexpectedEof
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
    )
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(terminate) => terminate,
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip_limiter_disabled() {
        let limiter = IpLimiter::new(0);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        let guards: Vec<_> = (0..100).filter_map(|_| limiter.try_acquire(ip)).collect();
        assert_eq!(guards.len(), 100);
        drop(guards);

        // Nothing was tracked, so the map stays empty.
        assert!(limiter.lock().is_empty());
    }

    #[test]
    fn test_ip_limiter_enforced_and_released() {
        let limiter = IpLimiter::new(2);
        let ip_a: IpAddr = "127.0.0.1".parse().unwrap();
        let ip_b: IpAddr = "10.0.0.1".parse().unwrap();

        let first = limiter.try_acquire(ip_a).expect("first allowed");
        let second = limiter.try_acquire(ip_a).expect("second allowed");
        assert!(limiter.try_acquire(ip_a).is_none(), "third rejected");

        // A different IP is unaffected.
        let other = limiter.try_acquire(ip_b).expect("other IP allowed");

        drop(second);
        let third = limiter.try_acquire(ip_a).expect("slot freed by drop");

        drop((first, third, other));
        assert!(limiter.lock().is_empty(), "counters must be cleaned up");
    }

    #[test]
    fn test_should_log_is_throttled() {
        let counter = AtomicU64::new(0);
        assert!(should_log(&counter));
        for _ in 0..98 {
            assert!(!should_log(&counter));
        }
        assert!(should_log(&counter), "every 100th event is logged");
    }
}
