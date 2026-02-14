# Rust Proxy

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)
[![CI](https://github.com/jayhsudev/rust-proxy/actions/workflows/rust.yml/badge.svg)](https://github.com/jayhsudev/rust-proxy/actions/workflows/rust.yml)

[中文版本](README_zh-CN.md) | English

A high-performance, lightweight asynchronous proxy server supporting SOCKS5 and HTTP protocols, written in Rust. Designed with simplicity, security, and performance in mind.

## Features

- 🌐 **Multi-Protocol**: SOCKS5 (RFC 1928) and HTTP/HTTPS CONNECT proxy
- 🔍 **Auto Detection**: Automatically identifies SOCKS5 or HTTP by inspecting the first byte
- 🔐 **Authentication**: bcrypt-hashed passwords for both SOCKS5 (RFC 1929) and HTTP Basic auth
- 🚀 **Async I/O**: Built on Tokio with zero-copy bidirectional forwarding
- 📝 **Configurable**: TOML config file with full CLI override support
- 📋 **Rolling Logs**: log4rs with size-based file rotation and archiving
- 🚦 **Connection Limits**: Semaphore-based concurrency control with configurable timeout

## Quick Start

```bash
# 1. Clone and build
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy
cargo build --release

# 2. Copy example config
cp config.example.toml config.toml

# 3. Run
./target/release/rust-proxy

# 4. Test (default: 127.0.0.1:1080)
curl -x socks5://127.0.0.1:1080 https://httpbin.org/ip
curl -x http://127.0.0.1:1080 https://httpbin.org/ip
```

## Installation

### Prerequisites

- Rust 1.70+ (2021 edition)
- Cargo

### Build from Source

```bash
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy
cargo build --release
./target/release/rust-proxy
```

### Run Tests

```bash
cargo test
cargo test -- --nocapture
cargo clippy
```

## Usage

### CLI Options

All CLI flags override values from the config file.

```bash
./rust-proxy                                    # default config.toml
./rust-proxy --config path/to/config.toml       # custom config
./rust-proxy --listen-address 0.0.0.0:8080      # override listen address
./rust-proxy --log-level debug                  # trace, debug, info, warn, error
./rust-proxy --buffer-size 8192                 # network buffer size in bytes
./rust-proxy --max-connections 2048             # concurrent connection limit
./rust-proxy --connect-timeout 15               # target server timeout in seconds
./rust-proxy --help
./rust-proxy --version
```

### Configuration File

Create `config.toml` (or copy from `config.example.toml`):

```toml
listen_address = "127.0.0.1:1080"

# Optional — remove this section to disable authentication
[users]
alice = "password123"
bob = "securepass"

[log]
level = "Info"
path = "logs/rust-proxy.log"
archive_pattern = "logs/archive/rust-proxy-{}.log"
file_count = 5
file_size = 10

buffer_size = 4096
max_connections = 1024
connect_timeout = 10
```

### Configuration Reference

| Option | Default | Description |
|--------|---------|-------------|
| `listen_address` | `127.0.0.1:1080` | Address and port to listen on |
| `users` | `{}` (empty) | Username/password pairs; empty = no auth |
| `log.level` | `Info` | Off, Error, Warn, Info, Debug, Trace |
| `log.path` | `logs/rust-proxy.log` | Log file path |
| `log.archive_pattern` | `logs/archive/rust-proxy-{}.log` | Archive file pattern (`{}` = index) |
| `log.file_count` | `5` | Number of archived log files to keep |
| `log.file_size` | `10` | Max size per log file (MB) |
| `buffer_size` | `4096` | Network buffer size in bytes (1–65536) |
| `max_connections` | `1024` | Max concurrent connections |
| `connect_timeout` | `10` | Timeout connecting to target servers (seconds) |

## Client Configuration

### curl

```bash
# SOCKS5
curl -x socks5://127.0.0.1:1080 https://httpbin.org/ip

# SOCKS5 with auth
curl -x socks5://alice:password123@127.0.0.1:1080 https://httpbin.org/ip

# HTTP
curl -x http://127.0.0.1:1080 https://httpbin.org/ip

# HTTP with auth
curl -x http://alice:password123@127.0.0.1:1080 https://httpbin.org/ip
```

### Environment Variables

```bash
export http_proxy=http://127.0.0.1:1080
export https_proxy=http://127.0.0.1:1080
export ALL_PROXY=socks5://127.0.0.1:1080
```

### Browser

**Firefox**: Settings → Network Settings → Manual proxy → SOCKS Host `127.0.0.1`, Port `1080`, SOCKS v5, check "Proxy DNS when using SOCKS v5".

**Chrome**:
```bash
google-chrome --proxy-server="socks5://127.0.0.1:1080"
google-chrome --proxy-server="http://127.0.0.1:1080"
```

### Git

```bash
git config --global http.proxy socks5://127.0.0.1:1080
git config --global https.proxy socks5://127.0.0.1:1080

# Remove
git config --global --unset http.proxy
git config --global --unset https.proxy
```

## Deployment

### systemd (Linux)

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

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now rust-proxy
sudo systemctl status rust-proxy
```

### Docker

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

```bash
docker build -t rust-proxy .
docker run -d -p 1080:1080 -v $(pwd)/config.toml:/app/config.toml rust-proxy
```

### Background (Unix)

```bash
nohup ./rust-proxy > proxy.out 2>&1 &
pkill rust-proxy   # stop
```

## Project Structure

```
rust-proxy/
├── src/
│   ├── main.rs              # Entry point, CLI args, fallback logger
│   ├── bin/
│   │   └── test_socks5.rs   # Standalone SOCKS5 handshake smoke test
│   ├── common/
│   │   ├── mod.rs
│   │   ├── auth.rs          # bcrypt password hashing and verification
│   │   ├── config.rs        # TOML config parsing and validation
│   │   └── logger.rs        # log4rs setup with rolling file appender
│   ├── net/
│   │   ├── mod.rs
│   │   └── conn.rs          # BufferedConnection with AsyncRead/AsyncWrite
│   └── proxy/
│       ├── mod.rs
│       ├── tcp.rs            # Listener, protocol detection, concurrency control
│       ├── socks5.rs         # SOCKS5 protocol (RFC 1928 / RFC 1929)
│       ├── http.rs           # HTTP CONNECT tunnel and plain HTTP forwarding
│       └── forward.rs        # Address resolution, timeout connect, bidirectional copy
├── config.example.toml
├── config.toml
├── Cargo.toml
├── Cargo.lock
├── LICENSE
├── README.md
└── README_zh-CN.md
```

## Protocol Support

### SOCKS5 (RFC 1928)

| Feature | Detail |
|---------|--------|
| Command | CONNECT (`0x01`) |
| Address types | IPv4 (`0x01`), Domain (`0x03`), IPv6 (`0x04`) |
| Auth methods | No auth (`0x00`), Username/Password (`0x02`, RFC 1929) |

When no users are configured the server also accepts clients that only offer method `0x02` — authentication succeeds automatically.

### HTTP Proxy

| Feature | Detail |
|---------|--------|
| CONNECT | HTTPS tunneling via bidirectional forwarding |
| GET / POST / PUT / DELETE / HEAD / OPTIONS / PATCH | Plain HTTP forwarding with hop-by-hop header stripping |
| Auth | `Proxy-Authorization: Basic` with proper `407` responses |

For non-CONNECT requests, headers are forwarded preserving original order and case. `Connection: close` is injected and the response is copied unidirectionally (target → client).

## Security Considerations

1. **Passwords** are bcrypt-hashed at startup — plaintext is never stored in memory after init
2. **Default bind** is `127.0.0.1` (localhost only); use `0.0.0.0` with caution
3. **No TLS** — the proxy does not encrypt traffic itself; rely on HTTPS at the application layer or wrap with a VPN / SSH tunnel
4. **Connection limits** prevent resource exhaustion; tune `max_connections` and `LimitNOFILE` for production

## Dependencies

| Crate | Purpose |
|-------|---------|
| [tokio](https://crates.io/crates/tokio) | Async runtime |
| [clap](https://crates.io/crates/clap) | CLI argument parsing |
| [serde](https://crates.io/crates/serde) | Serialization / deserialization |
| [config](https://crates.io/crates/config) | Configuration file handling |
| [log](https://crates.io/crates/log) | Logging facade |
| [log4rs](https://crates.io/crates/log4rs) | Logging with rolling file rotation |
| [thiserror](https://crates.io/crates/thiserror) | Ergonomic error types |
| [bcrypt](https://crates.io/crates/bcrypt) | Password hashing |
| [base64](https://crates.io/crates/base64) | Base64 encoding / decoding |
| [url](https://crates.io/crates/url) | URL parsing |

## Performance Tips

1. Increase `buffer_size` (e.g. `16384`) for high-throughput workloads
2. Raise OS file descriptor limits (`ulimit -n`) for many concurrent connections
3. Always build with `cargo build --release` for production
4. Use log level `Warn` or `Info` in production — `Debug` / `Trace` add measurable overhead

## Troubleshooting

**Port already in use**
```bash
lsof -i :1080
ss -tlnp | grep 1080
```

**Connection refused** — verify the proxy is running, the listen address matches your client config, and firewall rules allow the port.

**Authentication failures** — credentials are case-sensitive; ensure `[users]` exists in `config.toml` and your client sends auth.

**Debug mode**
```bash
./rust-proxy --log-level debug
tail -f logs/rust-proxy.log
```

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

MIT — see [LICENSE](LICENSE) for details.