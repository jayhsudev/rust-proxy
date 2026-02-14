use crate::common::auth::AuthManager;
use crate::common::config::Config;
use crate::common::logger;
use crate::proxy::tcp::TcpProxy;
use clap::Parser;
use log::LevelFilter;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

mod common;
mod net;
mod proxy;

/// Fallback logger that writes to stderr when log4rs fails to initialise.
struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= LevelFilter::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, value_name = "FILE", default_value = "config.toml")]
    config: String,

    #[arg(long, value_name = "ADDRESS")]
    listen_address: Option<String>,

    #[arg(short, long, value_name = "LEVEL", default_value = "info")]
    log_level: String,

    #[arg(long, value_name = "SIZE")]
    buffer_size: Option<usize>,

    #[arg(long, value_name = "COUNT")]
    max_connections: Option<usize>,

    #[arg(long, value_name = "SECONDS")]
    connect_timeout: Option<u64>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let mut config = match Config::from_file(&args.config) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to load config from {}: {}", args.config, e);
            std::process::exit(1);
        }
    };

    if let Some(listen_address) = args.listen_address {
        config.listen_address = listen_address;
    }
    if args.log_level.to_lowercase() != config.log.level.to_lowercase() {
        config.log.level = args.log_level;
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

    if let Err(e) = config.validate() {
        eprintln!("Invalid configuration: {}", e);
        std::process::exit(1);
    }

    if let Err(e) = logger::setup_logger(config.log.clone()) {
        eprintln!("Failed to initialize logger: {}", e);
        log::set_boxed_logger(Box::new(SimpleLogger)).unwrap();
        log::set_max_level(LevelFilter::Info);
    }

    log::info!("Starting with config: {:?}", config);

    let auth_manager = match AuthManager::new(&config.users) {
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

    println!("Proxy server listening on {}", config.listen_address);
    println!("Supporting SOCKS5 and HTTP proxy protocols");

    let proxy = TcpProxy::new(
        auth_manager,
        config.buffer_size,
        config.max_connections,
        Duration::from_secs(config.connect_timeout),
    );

    proxy.run(listener).await;
}
