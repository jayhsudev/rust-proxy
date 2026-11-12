use crate::common::auth::{AuthManager, AuthOptions};
use crate::common::config::Config;
use crate::common::log_throttle;
use crate::common::logger;
use crate::common::metrics::Metrics;
use crate::proxy::forward::Upstream;
use crate::proxy::options::ProxyOptions;
use crate::proxy::tcp::TcpProxy;
use clap::Parser;
use log::LevelFilter;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

mod common;
mod net;
mod proxy;

/// Fallback logger that writes to stderr when log4rs fails to initialise.
struct SimpleLogger {
    level: LevelFilter,
}

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

#[derive(Parser, Debug)]
#[command(author, version, about = "A lightweight proxy server supporting SOCKS5 and HTTP protocols", long_about = None)]
struct Args {
    /// Path to the configuration file
    #[arg(short, long, value_name = "FILE", default_value = "config.toml")]
    config: String,

    /// Address and port to listen on (e.g. 127.0.0.1:1080)
    #[arg(long, value_name = "ADDRESS")]
    listen_address: Option<String>,

    /// Logging level [trace, debug, info, warn, error, off]
    /// Defaults to the value from the configuration file.
    #[arg(short, long, value_name = "LEVEL")]
    log_level: Option<String>,

    /// Read/write buffer size in bytes (1–65536)
    #[arg(long, value_name = "SIZE")]
    buffer_size: Option<usize>,

    /// Maximum number of concurrent connections
    #[arg(long, value_name = "COUNT")]
    max_connections: Option<usize>,

    /// Timeout in seconds for connecting to target servers
    #[arg(long, value_name = "SECONDS")]
    connect_timeout: Option<u64>,

    /// Timeout in seconds for the client handshake / request head
    #[arg(long, value_name = "SECONDS")]
    handshake_timeout: Option<u64>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let mut config = match Config::from_file(&args.config) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to load config from '{}': {}", args.config, e);
            eprintln!("Hint: copy config.example.toml to config.toml and adjust it.");
            std::process::exit(1);
        }
    };

    if let Some(listen_address) = args.listen_address {
        config.listen_address = listen_address;
    }
    if let Some(log_level) = args.log_level {
        config.log.level = log_level;
    }
    if let Some(buffer_size) = args.buffer_size {
        config.buffer_size = buffer_size;
    }
    if let Some(max_connections) = args.max_connections {
        config.max_connections = max_connections;
    }
    if let Some(connect_timeout) = args.connect_timeout {
        config.connect_timeout = connect_timeout;
    }
    if let Some(handshake_timeout) = args.handshake_timeout {
        config.handshake_timeout = handshake_timeout;
    }

    if let Err(e) = config.validate() {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    if let Err(e) = logger::setup_logger(config.log.clone()) {
        eprintln!("Failed to initialize logger: {}", e);

        let level = LevelFilter::from_str(config.log.level.trim()).unwrap_or(LevelFilter::Info);
        if let Err(set_err) = log::set_boxed_logger(Box::new(SimpleLogger { level })) {
            eprintln!("Failed to install fallback logger: {}", set_err);
        }
        log::set_max_level(level);
    }

    // Remote input must not be able to flood the log file.
    log_throttle::configure(config.log.warn_rate_limit);

    // Never log credentials: only a redacted summary.
    log::info!("Configuration: {}", config.summary());

    let auth_manager = match AuthManager::with_options(
        &config.users,
        AuthOptions {
            cache_ttl: Duration::from_secs(config.auth_cache_ttl),
            max_concurrency: config.max_auth_concurrency,
        },
    ) {
        Ok(manager) => Arc::new(manager),
        Err(e) => {
            log::error!("Failed to create auth manager: {}", e);
            std::process::exit(1);
        }
    };

    let listener = match TcpListener::bind(&config.listen_address).await {
        Ok(listener) => listener,
        Err(e) => {
            log::error!("Failed to bind to {}: {}", config.listen_address, e);
            std::process::exit(1);
        }
    };

    let local_addr = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| config.listen_address.clone());

    println!("Proxy server listening on {}", local_addr);
    println!(
        "Protocols: SOCKS5 + HTTP, authentication: {}",
        if config.auth_enabled() {
            format!("enabled ({} user(s))", auth_manager.user_count())
        } else {
            "disabled".to_string()
        }
    );

    let options = Arc::new(ProxyOptions {
        buffer_size: config.buffer_size,
        relay_buffer_size: config.relay_buffer_size,
        connect_timeout: Duration::from_secs(config.connect_timeout),
        handshake_timeout: Duration::from_secs(config.handshake_timeout),
        keepalive: (config.tcp_keepalive > 0).then(|| Duration::from_secs(config.tcp_keepalive)),
        dns_cache_ttl: (config.dns_cache_ttl > 0)
            .then(|| Duration::from_secs(config.dns_cache_ttl)),
        upstream_keepalive: config.upstream_keepalive,
        upstream_pool_size: config.upstream_pool_size,
        upstream_pool_idle_timeout: Duration::from_secs(config.upstream_idle_timeout),
    });
    let upstream = Arc::new(Upstream::new((*options).clone()));
    let metrics = Metrics::new();

    if !config.metrics_address.trim().is_empty() {
        match TcpListener::bind(&config.metrics_address).await {
            Ok(metrics_listener) => {
                let address = metrics_listener
                    .local_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|_| config.metrics_address.clone());
                println!("Metrics endpoint listening on http://{address}/metrics");

                let metrics = metrics.clone();
                tokio::spawn(async move {
                    crate::common::metrics::serve(metrics_listener, metrics).await;
                });
            }
            Err(e) => {
                log::error!(
                    "Failed to bind metrics endpoint to {}: {}",
                    config.metrics_address,
                    e
                );
                std::process::exit(1);
            }
        }
    }

    let proxy = TcpProxy::new(
        auth_manager,
        upstream,
        options,
        metrics,
        config.max_connections,
        config.max_connections_per_ip,
    );

    proxy.run(listener).await;

    // Drain the asynchronous log writer before exiting.
    log::logger().flush();
}
