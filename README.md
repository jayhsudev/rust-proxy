# Rust Proxy

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2021-orange.svg)](https://www.rust-lang.org)
[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)](#)

[中文版本](README_zh-CN.md) | English

A high-performance, lightweight asynchronous proxy server supporting SOCKS5, HTTP, and TCP protocols, written in Rust. Designed with simplicity, security, and performance in mind.

## Quick Start

```bash
# 1. Clone and build
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy
cargo build --release

# 2. Copy example config (optional)
cp config.example.toml config.toml

# 3. Run the proxy server
./target/release/rust-proxy

# 4. Use the proxy (default: localhost:1080)
# Configure your applications to use SOCKS5 proxy at 127.0.0.1:1080
```

## Features

- 🔌 **Multiple Protocol Support**: SOCKS5, HTTP, and TCP proxy protocols
- 🔒 **User Authentication**: Secure user validation with bcrypt password hashing
- 🔧 **Highly Configurable**: Listening address, log level, buffer size, and more
- 📝 **TOML Configuration**: Easy-to-use configuration file format
- 🔐 **TLS Support**: Secure connections with native-tls
- 🚀 **High Performance**: Asynchronous design using Tokio runtime
- 📊 **Advanced Logging**: Comprehensive logging system with log4rs and file rotation
- 💾 **Memory Efficient**: Configurable buffer sizes and connection handling

## Installation

### Prerequisites
- Rust 1.70 or higher (2021 edition)
- Cargo (Rust package manager)

### Build from Source

```bash
# Clone the repository
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy

# Build the project with release optimizations
cargo build --release

# The binary will be in target/release/
# You can run it with:
./target/release/rust-proxy
```

## Usage

### Command Line Options

```bash
# Run with default configuration
./rust-proxy

# Specify a custom configuration file
./rust-proxy --config path/to/config.toml

# Specify a listening address
./rust-proxy --listen-address 127.0.0.1:1080

# Set log level (trace, debug, info, warn, error)
./rust-proxy --log-level debug

# Set buffer size in bytes
./rust-proxy --buffer-size 8192
```

### Configuration File Format

Create a `config.toml` file with the following format:

```toml
# The address and port the proxy server will listen on
listen_address = "127.0.0.1:1080"

# Users for authentication (optional, remove if not needed)
[users]
username1 = "password1"
username2 = "password2"

# Logging configuration
[log]
level = "info"                                    # Log level (trace, debug, info, warn, error)
path = "logs/rust-proxy.log"                      # Log file path
archive_pattern = "logs/archive/rust-proxy-{}.log" # Archive pattern
file_count = 5                                    # Number of log files to keep
file_size = 10                                    # Max file size in MB

# Buffer size in bytes for network operations
buffer_size = 4096
```

### Example Usage Scenarios

```bash
# Basic usage with default configuration
./rust-proxy

# Custom configuration file and log level
./rust-proxy --config my_config.toml --log-level debug

# Run on a specific port with a larger buffer
./rust-proxy --listen-address 0.0.0.0:3128 --buffer-size 16384

# Run as a SOCKS5 proxy on port 1080
./rust-proxy --listen-address 127.0.0.1:1080

# Run as an HTTP proxy on port 8080  
./rust-proxy --listen-address 127.0.0.1:8080
```

## Project Structure

```
rust-proxy/
├── src/
│   ├── main.rs         # Application entry point and CLI argument handling
│   ├── common/         # Common utilities and shared modules
│   │   ├── mod.rs      # Module declarations
│   │   ├── auth.rs     # User authentication and authorization logic
│   │   ├── config.rs   # Configuration file parsing and validation
│   │   ├── logger.rs   # Logging setup and configuration with log4rs
│   │   └── utils.rs    # Utility functions and helpers
│   ├── net/            # Network layer abstractions
│   │   ├── mod.rs      # Network module declarations
│   │   └── conn.rs     # Buffered connection handling and utilities
│   └── proxy/          # Proxy protocol implementations
│       ├── mod.rs      # Proxy module declarations and exports
│       ├── tcp.rs      # Raw TCP proxy implementation
│       ├── socks5.rs   # SOCKS5 proxy protocol handler
│       ├── http.rs     # HTTP/HTTPS proxy protocol handler
│       └── forward.rs  # Data forwarding and tunneling logic
├── target/             # Cargo build artifacts (generated)
├── .git/               # Git version control metadata
├── .gitignore          # Git ignore patterns
├── Cargo.toml          # Rust project manifest and dependencies
├── Cargo.lock          # Dependency lock file (generated)
├── config.toml         # User configuration file (created from example)
├── config.example.toml # Example configuration with comments
├── LICENSE             # MIT license file
├── README.md           # English documentation
└── README_zh-CN.md     # Chinese documentation
```

## Dependencies

- [tokio](https://crates.io/crates/tokio) - Asynchronous runtime for Rust
- [clap](https://crates.io/crates/clap) - Command line argument parsing with beautiful output
- [serde](https://crates.io/crates/serde) - Serialization and deserialization framework
- [config](https://crates.io/crates/config) - Configuration management library
- [log4rs](https://crates.io/crates/log4rs) - Flexible logging framework with file rotation
- [log](https://crates.io/crates/log) - Lightweight logging facade
- [native-tls](https://crates.io/crates/native-tls) - Native TLS implementation
- [tokio-native-tls](https://crates.io/crates/tokio-native-tls) - Tokio integration for native-tls
- [thiserror](https://crates.io/crates/thiserror) - Error handling with custom error types
- [bcrypt](https://crates.io/crates/bcrypt) - Password hashing function
- [base64](https://crates.io/crates/base64) - Base64 encoding and decoding
- [url](https://crates.io/crates/url) - URL parsing and manipulation

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request or open an Issue to improve this project.

## Acknowledgements

- Thanks to all the contributors who have helped make this project better
- Built with [Rust](https://www.rust-lang.org/) - a systems programming language that runs blazingly fast, prevents segfaults, and guarantees thread safety