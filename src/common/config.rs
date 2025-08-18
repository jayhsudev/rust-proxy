use config::ConfigError as ConfigLibError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// Configuration errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    FileReadError(#[from] std::io::Error),
    #[error("Failed to parse config file: {0}")]
    ParseError(String),
    #[error("Invalid config: {0}")]
    InvalidConfig(String),
    #[error("Config library error: {0}")]
    ConfigLibError(#[from] ConfigLibError),
}

/// Configuration structure
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Config {
    /// Listen address
    #[serde(default = "default_listen_address")]
    pub listen_address: String,
    /// User authentication information
    #[serde(default)]
    pub users: HashMap<String, String>,
    /// Log configuration
    #[serde(default)]
    pub log: LoggerConfig,
    /// Buffer size
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LoggerConfig {
    /// Log level
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Log file path
    #[serde(default = "default_log_path")]
    pub path: String,
    /// Log file archive pattern
    #[serde(default = "default_archive_pattern")]
    pub archive_pattern: String,
    /// Number of log files to keep
    #[serde(default = "default_file_count")]
    pub file_count: u32,
    /// Size of each log file (MB)
    #[serde(default = "default_file_size")]
    pub file_size: u64,
}

impl Default for LoggerConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            path: default_log_path(),
            archive_pattern: default_archive_pattern(),
            file_count: default_file_count(),
            file_size: default_file_size(),
        }
    }
}

/// Default listen address
fn default_listen_address() -> String {
    "127.0.0.1:1080".to_string()
}

/// Default log level
fn default_log_level() -> String {
    "Info".to_string()
}

/// Default log path
fn default_log_path() -> String {
    "logs/rust-proxy.log".to_string()
}

/// Default archive pattern
fn default_archive_pattern() -> String {
    "logs/archive/rust-proxy-{}.log".to_string()
}

/// Default file count
fn default_file_count() -> u32 {
    5
}

/// Default file size in MB
fn default_file_size() -> u64 {
    10
}

/// Default buffer size
fn default_buffer_size() -> usize {
    4096
}

impl Config {
    /// Load configuration from file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let settings = config::Config::builder()
            .add_source(config::File::from(path.as_ref()))
            .build()
            .map_err(ConfigError::ConfigLibError)?;

        let config: Config = settings
            .try_deserialize()
            .map_err(|e| ConfigError::ParseError(e.to_string()))?;

        config.validate()?;
        Ok(config)
    }

    /// Validate if configuration is valid
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate listen address format
        if self.listen_address.is_empty() {
            return Err(ConfigError::InvalidConfig(
                "Listen address cannot be empty".to_string(),
            ));
        }

        // Try to parse listen address to ensure correct format
        if let Err(_) = self.listen_address.parse::<std::net::SocketAddr>() {
            return Err(ConfigError::InvalidConfig(format!(
                "Invalid listen address format: {}",
                self.listen_address
            )));
        }

        // Validate buffer size
        if self.buffer_size == 0 || self.buffer_size > 65536 {
            return Err(ConfigError::InvalidConfig(format!(
                "Invalid buffer size: {}. Must be between 1 and 65536",
                self.buffer_size
            )));
        }

        Ok(())
    }
}
