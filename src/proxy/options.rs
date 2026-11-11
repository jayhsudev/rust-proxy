use std::time::Duration;

/// Runtime tuning shared by the SOCKS5 and HTTP proxy implementations.
#[derive(Debug, Clone)]
pub struct ProxyOptions {
    /// Handshake parsing buffer size.
    pub buffer_size: usize,
    /// Copy buffer size used while relaying tunnel traffic.
    pub relay_buffer_size: usize,
    /// Total budget for resolving and connecting to a target.
    pub connect_timeout: Duration,
    /// Budget for reading the client handshake / request head.
    pub handshake_timeout: Duration,
    /// TCP keepalive idle time for both endpoints; `None` disables it.
    pub keepalive: Option<Duration>,
    /// DNS cache TTL; `None` disables caching.
    pub dns_cache_ttl: Option<Duration>,
    /// Reuse upstream HTTP connections across client connections.
    pub upstream_keepalive: bool,
    /// Idle upstream connections kept per host.
    pub upstream_pool_size: usize,
    /// How long an idle upstream connection stays in the pool.
    pub upstream_pool_idle_timeout: Duration,
}

impl Default for ProxyOptions {
    fn default() -> Self {
        Self {
            buffer_size: 4096,
            relay_buffer_size: 32 * 1024,
            connect_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(10),
            keepalive: Some(Duration::from_secs(60)),
            dns_cache_ttl: Some(Duration::from_secs(60)),
            upstream_keepalive: true,
            upstream_pool_size: 8,
            upstream_pool_idle_timeout: Duration::from_secs(30),
        }
    }
}

impl ProxyOptions {
    /// Fast timeouts and no caching, suitable for loopback tests.
    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            handshake_timeout: Duration::from_secs(2),
            relay_buffer_size: 4096,
            dns_cache_ttl: None,
            ..Default::default()
        }
    }
}
