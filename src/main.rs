use crate::common::auth::AuthManager;
use crate::common::config::Config;
use crate::common::logger;
use crate::proxy::tcp::TcpProxy;
use clap::Parser;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;

mod common;
mod net;
mod proxy;

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
    let mut config = Config::from_file(&args.config).expect("Failed to load config");

    // Apply command line argument overrides
    if let Some(listen_address) = args.listen_address {
        config.listen_address = listen_address;
    }

    if args.log_level != "Info" {
        config.log.level = args.log_level;
    }

    if let Some(buffer_size) = args.buffer_size {
        config.buffer_size = buffer_size;
    }

    // Initialize logging system
    logger::setup_logger(config.log.clone()).expect("Failed to initialize logger");

    log::info!("Rust Proxy server starting with config: {:?}", config);

    // Create authentication manager
    let auth_manager = Arc::new(
        AuthManager::new(&config.users)
            .expect("Failed to create auth manager: invalid credentials format"),
    );

    // Create Tokio runtime
    let rt = Runtime::new().expect("Failed to create Tokio runtime");

    // Start TCP listening
    let listener = rt.block_on(async {
        TcpListener::bind(&config.listen_address)
            .await
            .expect(&format!(
                "Failed to bind to address: {}",
                config.listen_address
            ))
    });

    println!("Rust Proxy server listening on {}", config.listen_address);
    println!("Supporting SOCKS5 and HTTP proxy protocols");

    // Run proxy service
    rt.block_on(async move {
        let proxy = TcpProxy::new(auth_manager);
        proxy.run(listener).await;
    });
}
