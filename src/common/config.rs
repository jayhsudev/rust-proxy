use config::ConfigError as ConfigLibError;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    NotFound(String),
    #[error("Failed to parse config file: {0}")]
    ParseError(String),
    #[error("Invalid config: {0}")]
    InvalidConfig(String),
    #[error("Config library error: {0}")]
    ConfigLibError(#[from] ConfigLibError),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default = "default_listen_address")]
    pub listen_address: String,
    #[serde(default)]
    pub users: HashMap<String, String>,
    #[serde(default)]
    pub log: LoggerConfig,
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    /// Timeout in seconds for connecting to target servers
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: u64,
    /// Timeout in seconds for the client protocol handshake / request head
    #[serde(default = "default_handshake_timeout")]
    pub handshake_timeout: u64,
    /// TCP keepalive idle time in seconds (0 disables keepalive)
    #[serde(default = "default_tcp_keepalive")]
    pub tcp_keepalive: u64,
    /// Buffer size used while relaying tunnel traffic (1024-1048576)
    #[serde(default = "default_relay_buffer_size")]
    pub relay_buffer_size: usize,
    /// Concurrent connection limit per client IP (0 = unlimited)
    #[serde(default)]
    pub max_connections_per_ip: usize,
    /// DNS cache TTL in seconds (0 disables caching)
    #[serde(default = "default_cache_ttl")]
    pub dns_cache_ttl: u64,
    /// Successful authentication cache TTL in seconds (0 disables caching)
    #[serde(default = "default_cache_ttl")]
    pub auth_cache_ttl: u64,
    /// Max concurrent bcrypt verifications (0 = number of CPUs)
    #[serde(default)]
    pub max_auth_concurrency: usize,
    /// Reuse upstream HTTP connections (keep-alive) across client connections
    #[serde(default = "default_true")]
    pub upstream_keepalive: bool,
    /// Idle connections kept per upstream host (0 disables pooling)
    #[serde(default = "default_upstream_pool_size")]
    pub upstream_pool_size: usize,
    /// Seconds an idle upstream connection is kept before being dropped
    #[serde(default = "default_upstream_idle_timeout")]
    pub upstream_idle_timeout: u64,
    /// Address for the metrics/health endpoint (empty = disabled)
    #[serde(default)]
    pub metrics_address: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LoggerConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_path")]
    pub path: String,
    #[serde(default = "default_archive_pattern")]
    pub archive_pattern: String,
    #[serde(default = "default_file_count")]
    pub file_count: u32,
    /// Max file size in MB
    #[serde(default = "default_file_size")]
    pub file_size: u64,
    /// Write log records on a dedicated thread (bounded queue)
    #[serde(default = "default_true")]
    pub non_blocking: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_address: default_listen_address(),
            users: HashMap::new(),
            log: LoggerConfig::default(),
            buffer_size: default_buffer_size(),
            max_connections: default_max_connections(),
            connect_timeout: default_connect_timeout(),
            handshake_timeout: default_handshake_timeout(),
            tcp_keepalive: default_tcp_keepalive(),
            relay_buffer_size: default_relay_buffer_size(),
            max_connections_per_ip: 0,
            dns_cache_ttl: default_cache_ttl(),
            auth_cache_ttl: default_cache_ttl(),
            max_auth_concurrency: 0,
            upstream_keepalive: true,
            upstream_pool_size: default_upstream_pool_size(),
            upstream_idle_timeout: default_upstream_idle_timeout(),
            metrics_address: String::new(),
        }
    }
}

impl Default for LoggerConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            path: default_log_path(),
            archive_pattern: default_archive_pattern(),
            file_count: default_file_count(),
            file_size: default_file_size(),
            non_blocking: true,
        }
    }
}

fn default_listen_address() -> String {
    "127.0.0.1:1080".to_string()
}

fn default_log_level() -> String {
    "Info".to_string()
}

fn default_log_path() -> String {
    "logs/rust-proxy.log".to_string()
}

fn default_archive_pattern() -> String {
    "logs/archive/rust-proxy-{}.log".to_string()
}

fn default_file_count() -> u32 {
    5
}

fn default_file_size() -> u64 {
    10
}

fn default_buffer_size() -> usize {
    4096
}

fn default_max_connections() -> usize {
    1024
}

fn default_connect_timeout() -> u64 {
    10
}

fn default_handshake_timeout() -> u64 {
    10
}

fn default_tcp_keepalive() -> u64 {
    60
}

fn default_relay_buffer_size() -> usize {
    32 * 1024
}

fn default_cache_ttl() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

fn default_upstream_pool_size() -> usize {
    8
}

fn default_upstream_idle_timeout() -> u64 {
    30
}

impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(ConfigError::NotFound(path.display().to_string()));
        }

        let settings = config::Config::builder()
            .add_source(config::File::from(path))
            .build()?;

        let config: Config = settings
            .try_deserialize()
            .map_err(|e| ConfigError::ParseError(e.to_string()))?;

        Ok(config)
    }

    /// Whether client authentication is enabled.
    pub fn auth_enabled(&self) -> bool {
        !self.users.is_empty()
    }

    /// Human readable, credential-free summary suitable for logging.
    pub fn summary(&self) -> String {
        format!(
            "listen_address={}, auth={}, users={}, buffer_size={}, relay_buffer_size={}, \
             max_connections={}, max_connections_per_ip={}, connect_timeout={}s, \
             handshake_timeout={}s, tcp_keepalive={}s, dns_cache_ttl={}s, auth_cache_ttl={}s, \
             upstream_keepalive={}, upstream_pool_size={}, upstream_idle_timeout={}s, \
             metrics_address={}, log_level={}, log_path={}",
            self.listen_address,
            if self.auth_enabled() {
                "enabled"
            } else {
                "disabled"
            },
            self.users.len(),
            self.buffer_size,
            self.relay_buffer_size,
            self.max_connections,
            self.max_connections_per_ip,
            self.connect_timeout,
            self.handshake_timeout,
            self.tcp_keepalive,
            self.dns_cache_ttl,
            self.auth_cache_ttl,
            self.upstream_keepalive,
            self.upstream_pool_size,
            self.upstream_idle_timeout,
            if self.metrics_address.is_empty() {
                "disabled"
            } else {
                &self.metrics_address
            },
            self.log.level,
            self.log.path,
        )
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.listen_address.trim().is_empty() {
            return Err(ConfigError::InvalidConfig(
                "listen_address cannot be empty".to_string(),
            ));
        }

        if self.listen_address.parse::<std::net::SocketAddr>().is_err() {
            return Err(ConfigError::InvalidConfig(format!(
                "invalid listen_address '{}', expected HOST:PORT (e.g. 127.0.0.1:1080)",
                self.listen_address
            )));
        }

        if self.buffer_size == 0 || self.buffer_size > 65536 {
            return Err(ConfigError::InvalidConfig(format!(
                "invalid buffer_size {}, must be between 1 and 65536",
                self.buffer_size
            )));
        }

        if self.max_connections == 0 {
            return Err(ConfigError::InvalidConfig(
                "max_connections must be greater than 0".to_string(),
            ));
        }

        if self.connect_timeout == 0 {
            return Err(ConfigError::InvalidConfig(
                "connect_timeout must be greater than 0".to_string(),
            ));
        }

        if self.handshake_timeout == 0 {
            return Err(ConfigError::InvalidConfig(
                "handshake_timeout must be greater than 0".to_string(),
            ));
        }

        if !(1024..=1024 * 1024).contains(&self.relay_buffer_size) {
            return Err(ConfigError::InvalidConfig(format!(
                "invalid relay_buffer_size {}, must be between 1024 and 1048576",
                self.relay_buffer_size
            )));
        }

        if !self.metrics_address.trim().is_empty()
            && self
                .metrics_address
                .parse::<std::net::SocketAddr>()
                .is_err()
        {
            return Err(ConfigError::InvalidConfig(format!(
                "invalid metrics_address '{}', expected HOST:PORT or empty to disable",
                self.metrics_address
            )));
        }

        for (username, password) in &self.users {
            if username.trim().is_empty() {
                return Err(ConfigError::InvalidConfig(
                    "usernames must not be empty".to_string(),
                ));
            }
            if password.is_empty() {
                return Err(ConfigError::InvalidConfig(format!(
                    "password for user '{}' must not be empty",
                    username
                )));
            }
            if password.chars().any(|c| c.is_control()) {
                return Err(ConfigError::InvalidConfig(format!(
                    "password for user '{}' contains control characters",
                    username
                )));
            }
        }

        if LevelFilter::from_str(self.log.level.trim()).is_err() {
            return Err(ConfigError::InvalidConfig(format!(
                "invalid log level '{}', expected one of: off, error, warn, info, debug, trace",
                self.log.level
            )));
        }

        if self.log.path.trim().is_empty() {
            return Err(ConfigError::InvalidConfig(
                "log.path cannot be empty".to_string(),
            ));
        }

        if !self.log.archive_pattern.contains("{}") {
            return Err(ConfigError::InvalidConfig(format!(
                "log.archive_pattern '{}' must contain a '{{}}' placeholder",
                self.log.archive_pattern
            )));
        }

        if self.log.file_count == 0 {
            return Err(ConfigError::InvalidConfig(
                "log.file_count must be greater than 0".to_string(),
            ));
        }

        if self.log.file_size == 0 || self.log.file_size > 1024 * 1024 {
            return Err(ConfigError::InvalidConfig(format!(
                "invalid log.file_size {}, must be between 1 and 1048576 (MB)",
                self.log.file_size
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn write_temp_config(contents: &str) -> std::path::PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "rust-proxy-config-test-{}-{}.toml",
            std::process::id(),
            id
        ));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_defaults_are_valid() {
        let config = Config::default();
        assert_eq!(config.listen_address, "127.0.0.1:1080");
        assert!(!config.auth_enabled());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_from_file() {
        let path = write_temp_config(
            r#"
listen_address = "127.0.0.1:1081"
buffer_size = 8192
max_connections = 16
max_connections_per_ip = 4
connect_timeout = 3
handshake_timeout = 4
tcp_keepalive = 30
relay_buffer_size = 65536
dns_cache_ttl = 0
auth_cache_ttl = 5

[users]
alice = "secret"

[log]
level = "Debug"
"#,
        );

        let config = Config::from_file(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(config.listen_address, "127.0.0.1:1081");
        assert_eq!(config.buffer_size, 8192);
        assert_eq!(config.max_connections, 16);
        assert_eq!(config.max_connections_per_ip, 4);
        assert_eq!(config.connect_timeout, 3);
        assert_eq!(config.handshake_timeout, 4);
        assert_eq!(config.tcp_keepalive, 30);
        assert_eq!(config.relay_buffer_size, 65536);
        assert_eq!(config.dns_cache_ttl, 0);
        assert_eq!(config.auth_cache_ttl, 5);
        assert!(config.auth_enabled());
        assert_eq!(config.log.level, "Debug");
        assert_eq!(config.log.file_count, 5);
    }

    #[test]
    fn test_new_options_have_safe_defaults() {
        let config = Config::default();
        assert_eq!(config.tcp_keepalive, 60);
        assert_eq!(config.relay_buffer_size, 32 * 1024);
        assert_eq!(config.max_connections_per_ip, 0);
        assert_eq!(config.dns_cache_ttl, 60);
        assert_eq!(config.auth_cache_ttl, 60);
        assert_eq!(config.max_auth_concurrency, 0);
    }

    #[test]
    fn test_missing_file() {
        let err = Config::from_file("definitely-not-here.toml").unwrap_err();
        assert!(matches!(err, ConfigError::NotFound(_)));
    }

    #[test]
    fn test_summary_hides_passwords() {
        let mut config = Config::default();
        config
            .users
            .insert("alice".to_string(), "topsecret".to_string());
        let summary = config.summary();
        assert!(!summary.contains("topsecret"));
        assert!(summary.contains("auth=enabled"));
    }

    #[test]
    fn test_invalid_values() {
        let invalid = [
            Config {
                buffer_size: 0,
                ..Default::default()
            },
            Config {
                buffer_size: 65537,
                ..Default::default()
            },
            Config {
                max_connections: 0,
                ..Default::default()
            },
            Config {
                connect_timeout: 0,
                ..Default::default()
            },
            Config {
                handshake_timeout: 0,
                ..Default::default()
            },
            Config {
                relay_buffer_size: 0,
                ..Default::default()
            },
            Config {
                relay_buffer_size: 1024 * 1024 + 1,
                ..Default::default()
            },
            Config {
                listen_address: "not-an-address".to_string(),
                ..Default::default()
            },
            Config {
                users: HashMap::from([("alice".to_string(), String::new())]),
                ..Default::default()
            },
            Config {
                users: HashMap::from([("  ".to_string(), "secret".to_string())]),
                ..Default::default()
            },
            Config {
                log: LoggerConfig {
                    level: "verbose".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            Config {
                log: LoggerConfig {
                    archive_pattern: "logs/archive.log".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            Config {
                log: LoggerConfig {
                    file_size: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            Config {
                log: LoggerConfig {
                    file_count: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
        ];

        for config in invalid {
            assert!(
                config.validate().is_err(),
                "expected invalid config: {:?}",
                config.summary()
            );
        }
    }
}
