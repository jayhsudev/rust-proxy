use crate::common::auth::AuthManager;
use crate::common::config::Config;
use crate::common::logger;
use crate::proxy::tcp::TcpProxy;
use clap::Parser;
use log::LevelFilter;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;

mod common;
mod net;
mod proxy;

/// Simple logger that logs to stderr, used as fallback when log4rs fails
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

// Define command line arguments structure
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Sets a custom config file
    #[arg(short, long, value_name = "FILE", default_value = "config.toml")]
    config: String,

    /// Sets the listening address (overrides config file)
    #[arg(long, value_name = "ADDRESS")]
    listen_address: Option<String>,

    /// Sets the log level (trace, debug, info, warn, error)
    #[arg(short, long, value_name = "LEVEL", default_value = "info")]
    log_level: String,

    /// Sets the buffer size in bytes (overrides config file)
    #[arg(long, value_name = "SIZE")]
    buffer_size: Option<usize>,
}

fn main() {
    // Parse command line arguments
    let args = Args::parse();

    // Load configuration
    let mut config = match Config::from_file(&args.config) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to load config from {}: {}", args.config, e);
            std::process::exit(1);
        }
    };

    // Apply command line argument overrides
    if let Some(listen_address) = args.listen_address {
        config.listen_address = listen_address;
    }

    if args.log_level.to_lowercase() != config.log.level.to_lowercase() {
        config.log.level = args.log_level;
    }

    if let Some(buffer_size) = args.buffer_size {
        config.buffer_size = buffer_size;
    }

    // Validate configuration after overrides
    if let Err(e) = config.validate() {
        log::error!("Invalid configuration after command line overrides: {}", e);
        eprintln!("Invalid configuration: {}", e);
        std::process::exit(1);
    }

    // Initialize logging system
    if let Err(e) = logger::setup_logger(config.log.clone()) {
        eprintln!("Failed to initialize logger: {}", e);
        // Fallback to basic console logging
        log::set_boxed_logger(Box::new(SimpleLogger)).unwrap();
        log::set_max_level(LevelFilter::Info);
    }

    log::info!("Rust Proxy server starting with config: {:?}", config);

    // Create authentication manager
    let auth_manager = match AuthManager::new(&config.users) {
        Ok(manager) => Arc::new(manager),
        Err(e) => {
            log::error!("Failed to create auth manager: {}", e);
            eprintln!("Failed to create auth manager: invalid credentials format");
            std::process::exit(1);
        }
    };

    // Create Tokio runtime
    let rt = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("Failed to create Tokio runtime: {}", e);
            eprintln!("Failed to create Tokio runtime");
            std::process::exit(1);
        }
    };

    // Start TCP listening
    let listener = match rt.block_on(async { TcpListener::bind(&config.listen_address).await }) {
        Ok(listener) => listener,
        Err(e) => {
            log::error!("Failed to bind to address {}: {}", config.listen_address, e);
            eprintln!("Failed to bind to address: {}", config.listen_address);
            std::process::exit(1);
        }
    };

    println!("Rust Proxy server listening on {}", config.listen_address);
    println!("Supporting SOCKS5 and HTTP proxy protocols");

    // Run proxy service
    rt.block_on(async move {
        let proxy = TcpProxy::new(auth_manager);
        proxy.run(listener).await;
    });
}
