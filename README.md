# Rust Proxy

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2021-orange.svg)](https://www.rust-lang.org)
[![CI](https://github.com/jayhsudev/rust-proxy/actions/workflows/rust.yml/badge.svg)](https://github.com/jayhsudev/rust-proxy/actions/workflows/rust.yml)

[中文版本](README_zh-CN.md) | English

A high-performance, lightweight asynchronous proxy server supporting SOCKS5, HTTP, and TCP protocols, written in Rust. Designed with simplicity, security, and performance in mind.

## Features

- 🔌 **Multiple Protocol Support**: SOCKS5 (v5), HTTP, and HTTPS CONNECT proxy protocols
- 🔒 **User Authentication**: Secure user validation with bcrypt password hashing
- 🔧 **Highly Configurable**: Listening address, log level, buffer size, and more
- 📝 **TOML Configuration**: Easy-to-use configuration file format
- 🚀 **High Performance**: Asynchronous design using Tokio runtime
- 📊 **Advanced Logging**: Comprehensive logging system with log4rs and file rotation
- 💾 **Memory Efficient**: Configurable buffer sizes and connection handling
- 🔄 **Auto Protocol Detection**: Automatically detects SOCKS5 or HTTP protocol

## Quick Start

```bash
# 1. Clone and build
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy
cargo build --release

# 2. Copy example config (required)
cp config.example.toml config.toml

# 3. Run the proxy server
./target/release/rust-proxy

# 4. Use the proxy (default: localhost:1080)
# Configure your applications to use SOCKS5 proxy at 127.0.0.1:1080
```

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
./target/release/rust-proxy
```

### Run Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run clippy for linting
cargo clippy
```

## Usage

### Command Line Options

```bash
# Run with default configuration
./rust-proxy

# Specify a custom configuration file
./rust-proxy --config path/to/config.toml

# Specify a listening address (overrides config file)
./rust-proxy --listen-address 127.0.0.1:1080

# Set log level (trace, debug, info, warn, error)
./rust-proxy --log-level debug

# Set buffer size in bytes (overrides config file)
./rust-proxy --buffer-size 8192

# Set maximum number of concurrent connections (overrides config file)
./rust-proxy --max-connections 2048

# Set connection timeout in seconds for target servers (overrides config file)
./rust-proxy --connect-timeout 15

# Show help
./rust-proxy --help

# Show version
./rust-proxy --version
```

### Configuration File

Create a `config.toml` file (or copy from `config.example.toml`):

```toml
# The address and port the proxy server will listen on
listen_address = "127.0.0.1:1080"

# Users for authentication (optional, remove section for no auth)
[users]
alice = "password123"
bob = "securepass"

# Logging configuration
[log]
level = "Info"                                    # Off, Error, Warn, Info, Debug, Trace
path = "logs/rust-proxy.log"                      # Log file path
archive_pattern = "logs/archive/rust-proxy-{}.log" # Archive pattern
file_count = 5                                    # Number of log files to keep
file_size = 10                                    # Max file size in MB

# Buffer size in bytes for network operations
buffer_size = 4096
```

### Configuration Options

| Option | Default | Description |
|--------|---------|-------------|
| `listen_address` | `127.0.0.1:1080` | Address and port to listen on |
| `users` | `{}` (empty) | Username/password pairs for authentication |
| `log.level` | `Info` | Log level: Off, Error, Warn, Info, Debug, Trace |
| `log.path` | `logs/rust-proxy.log` | Path to the log file |
| `log.archive_pattern` | `logs/archive/rust-proxy-{}.log` | Pattern for archived logs |
| `log.file_count` | `5` | Number of archived log files to retain |
| `log.file_size` | `10` | Max size per log file in MB |
| `buffer_size` | `4096` | Network buffer size (1-65536 bytes) |
| `max_connections` | `1024` | Maximum number of concurrent connections |
| `connect_timeout` | `10` | Connection timeout in seconds for target servers |

## Client Configuration

### Using curl

```bash
# SOCKS5 proxy
curl -x socks5://127.0.0.1:1080 https://httpbin.org/ip

# SOCKS5 proxy with authentication
curl -x socks5://alice:password123@127.0.0.1:1080 https://httpbin.org/ip

# HTTP proxy
curl -x http://127.0.0.1:1080 https://httpbin.org/ip

# HTTP proxy with authentication
curl -x http://alice:password123@127.0.0.1:1080 https://httpbin.org/ip
```

### Using wget

```bash
# HTTP proxy
https_proxy=http://127.0.0.1:1080 wget https://httpbin.org/ip

# With authentication
https_proxy=http://alice:password123@127.0.0.1:1080 wget https://httpbin.org/ip
```

### Environment Variables

Set these environment variables to use the proxy system-wide:

```bash
# For HTTP/HTTPS proxy
export http_proxy=http://127.0.0.1:1080
export https_proxy=http://127.0.0.1:1080

# For SOCKS5 proxy (application dependent)
export ALL_PROXY=socks5://127.0.0.1:1080
```

### Browser Configuration

#### Firefox
1. Open Settings → Network Settings → Settings
2. Select "Manual proxy configuration"
3. For SOCKS5: Set SOCKS Host to `127.0.0.1` and Port to `1080`
4. Select "SOCKS v5"
5. Check "Proxy DNS when using SOCKS v5"

#### Chrome/Chromium
```bash
# Launch with SOCKS5 proxy
google-chrome --proxy-server="socks5://127.0.0.1:1080"

# Launch with HTTP proxy
google-chrome --proxy-server="http://127.0.0.1:1080"
```

### Git Configuration

```bash
# SOCKS5 proxy for Git
git config --global http.proxy socks5://127.0.0.1:1080
git config --global https.proxy socks5://127.0.0.1:1080

# HTTP proxy for Git
git config --global http.proxy http://127.0.0.1:1080
git config --global https.proxy http://127.0.0.1:1080

# Remove proxy settings
git config --global --unset http.proxy
git config --global --unset https.proxy
```

## Deployment

### Running as a systemd Service (Linux)

Create `/etc/systemd/system/rust-proxy.service`:

```ini
[Unit]
Description=Rust Proxy Server
After=network.target

[Service]
Type=simple
User=nobody
Group=nogroup
WorkingDirectory=/opt/rust-proxy
ExecStart=/opt/rust-proxy/rust-proxy --config /opt/rust-proxy/config.toml
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

Then enable and start the service:

```bash
sudo systemctl daemon-reload
sudo systemctl enable rust-proxy
sudo systemctl start rust-proxy
sudo systemctl status rust-proxy
```

### Running with Docker

Create a `Dockerfile`:

```dockerfile
FROM rust:1.75-alpine AS builder
WORKDIR /app
COPY . .
RUN apk add --no-cache musl-dev && cargo build --release

FROM alpine:latest
RUN apk add --no-cache ca-certificates
WORKDIR /app
COPY --from=builder /app/target/release/rust-proxy .
COPY config.example.toml config.toml
EXPOSE 1080
CMD ["./rust-proxy"]
```

Build and run:

```bash
docker build -t rust-proxy .
docker run -d -p 1080:1080 -v $(pwd)/config.toml:/app/config.toml rust-proxy
```

### Running in Background (Unix)

```bash
# Using nohup
nohup ./rust-proxy > proxy.out 2>&1 &

# Check if running
ps aux | grep rust-proxy

# Stop the proxy
pkill rust-proxy
```

## Project Structure

```
rust-proxy/
├── src/
│   ├── main.rs           # Application entry point and CLI argument handling
│   ├── common/           # Common utilities and shared modules
│   │   ├── mod.rs        # Module declarations
│   │   ├── auth.rs       # User authentication with bcrypt password hashing
│   │   ├── config.rs     # Configuration file parsing and validation
│   │   ├── logger.rs     # Logging setup with log4rs and file rotation
│   │   └── utils.rs      # Utility functions (base64 encoding, etc.)
│   ├── net/              # Network layer abstractions
│   │   ├── mod.rs        # Network module declarations
│   │   └── conn.rs       # Buffered connection handling with async I/O
│   └── proxy/            # Proxy protocol implementations
│       ├── mod.rs        # Proxy module declarations and exports
│       ├── tcp.rs        # TCP listener with auto protocol detection
│       ├── socks5.rs     # SOCKS5 proxy protocol (RFC 1928)
│       ├── http.rs       # HTTP/HTTPS CONNECT proxy handler
│       └── forward.rs    # Bidirectional data forwarding
├── .github/workflows/    # GitHub Actions CI/CD configurations
│   └── rust.yml          # Rust build, test, and lint workflow
├── Cargo.toml            # Rust project manifest and dependencies
├── Cargo.lock            # Dependency lock file
├── config.example.toml   # Example configuration with documentation
├── LICENSE               # MIT license file
├── README.md             # English documentation (this file)
└── README_zh-CN.md       # Chinese documentation
```

## Protocol Support

### SOCKS5 (RFC 1928)

The proxy implements SOCKS5 protocol with the following features:

- **Commands**: CONNECT (0x01)
- **Address Types**: IPv4 (0x01), Domain name (0x03), IPv6 (0x04)
- **Authentication Methods**:
  - No authentication (0x00) - when no users configured
  - Username/Password (0x02) - RFC 1929

### HTTP Proxy

The proxy supports HTTP proxy protocol with:

- **CONNECT method**: For HTTPS tunneling
- **GET/POST/etc.**: For plain HTTP requests (forwarded to target)
- **Proxy-Authorization**: Basic authentication support

## Security Considerations

1. **Password Storage**: Passwords in config are hashed with bcrypt at startup
2. **Authentication**: Supports both SOCKS5 and HTTP Basic authentication
3. **Binding Address**: Default binds to `127.0.0.1` (localhost only)
   - Use `0.0.0.0` to accept external connections (use with caution)
4. **No Encryption**: The proxy itself doesn't encrypt traffic
   - Use HTTPS/TLS at the application level
   - Consider using a VPN or SSH tunnel for transport security

## Troubleshooting

### Common Issues

**Port already in use**
```bash
# Find process using the port
lsof -i :1080
# Or on Linux
ss -tlnp | grep 1080
```

**Permission denied on port < 1024**
```bash
# Either run as root (not recommended) or use a port >= 1024
./rust-proxy --listen-address 0.0.0.0:1080
```

**Connection refused**
- Ensure the proxy is running: `ps aux | grep rust-proxy`
- Check the listening address matches your client configuration
- Verify firewall rules allow the connection

**Authentication failures**
- Ensure username/password match exactly (case-sensitive)
- Check that the `[users]` section exists in config.toml
- For SOCKS5, ensure your client supports authentication

### Debug Mode

Run with debug logging to diagnose issues:

```bash
./rust-proxy --log-level debug
```

Check the log file for detailed information:

```bash
tail -f logs/rust-proxy.log
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| [tokio](https://crates.io/crates/tokio) | Asynchronous runtime |
| [clap](https://crates.io/crates/clap) | Command line argument parsing |
| [serde](https://crates.io/crates/serde) | Serialization/deserialization |
| [config](https://crates.io/crates/config) | Configuration management |
| [toml](https://crates.io/crates/toml) | TOML file parsing |
| [log4rs](https://crates.io/crates/log4rs) | Logging with file rotation |
| [log](https://crates.io/crates/log) | Logging facade |
| [thiserror](https://crates.io/crates/thiserror) | Error type definitions |
| [bcrypt](https://crates.io/crates/bcrypt) | Password hashing |
| [base64](https://crates.io/crates/base64) | Base64 encoding/decoding |
| [url](https://crates.io/crates/url) | URL parsing |

## Performance Tips

1. **Buffer Size**: Increase `buffer_size` for high-throughput scenarios (e.g., 16384)
2. **File Descriptors**: Increase system limits for many concurrent connections
3. **Release Build**: Always use `cargo build --release` for production
4. **Logging Level**: Use `Info` or `Warn` in production; `Debug`/`Trace` adds overhead

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request or open an Issue.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgements

- Built with [Rust](https://www.rust-lang.org/) - A language empowering everyone to build reliable and efficient software
- Thanks to all the contributors who have helped make this project better