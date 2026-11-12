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
- 🛡 **Hardened**: handshake timeouts, request head caps, streamed request bodies (constant memory), graceful shutdown
- ♻️ **Upstream keep-alive**: framed response parsing with a per-host connection pool
- 📊 **Observable**: optional Prometheus `/metrics` and `/healthz` endpoint
- 🧪 **Tested**: unit and protocol-level tests (SOCKS5, HTTP CONNECT/forward/auth/limits)

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

- Rust 1.88+ (2024 edition)
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
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

The suite runs entirely on loopback (no internet access required) and covers
config parsing/validation, bcrypt authentication (plaintext and pre-hashed
passwords, concurrency cap, credential cache), buffered connection framing and
splitting, SOCKS5 negotiation/auth/reply codes, HTTP CONNECT tunnelling,
streamed and chunked request bodies, early upstream responses, request
smuggling rejection, relay half-close propagation, per-IP limiting, the DNS
cache, HTTP/1.1 response framing (`Content-Length`/chunked/HEAD/204/interim
1xx), upstream connection pooling and reuse, the metrics endpoint and the
asynchronous log writer.

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
./rust-proxy --handshake-timeout 5              # client handshake timeout in seconds
./rust-proxy --help
./rust-proxy --version
```

### Configuration File

Create `config.toml` (or copy from `config.example.toml`):

> **TOML ordering matters** — top-level options must be declared *before* the
> first `[section]` header. A key placed after `[users]` or `[log]` is parsed as
> part of that section (e.g. `buffer_size` under `[users]` becomes a user
> account, silently enabling authentication).

```toml
listen_address = "127.0.0.1:1080"
buffer_size = 4096
relay_buffer_size = 32768
max_connections = 1024
max_connections_per_ip = 0
connect_timeout = 10
handshake_timeout = 10
tcp_keepalive = 60
dns_cache_ttl = 60
auth_cache_ttl = 60
max_auth_concurrency = 0
upstream_keepalive = true
upstream_pool_size = 8
upstream_idle_timeout = 30
metrics_address = "127.0.0.1:1081"

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
```

### Configuration Reference

| Option | Default | Description |
|--------|---------|-------------|
| `listen_address` | `127.0.0.1:1080` | Address and port to listen on |
| `users` | `{}` (empty) | Username/password pairs; empty = no auth. Values starting with `$2` are used as pre-hashed bcrypt passwords |
| `log.level` | `Info` | Off, Error, Warn, Info, Debug, Trace |
| `log.path` | `logs/rust-proxy.log` | Log file path |
| `log.archive_pattern` | `logs/archive/rust-proxy-{}.log` | Archive file pattern (`{}` = index, required) |
| `log.file_count` | `5` | Number of archived log files to keep |
| `log.file_size` | `10` | Max size per log file (MB) |
| `buffer_size` | `4096` | Handshake parsing buffer size in bytes (1–65536) |
| `relay_buffer_size` | `32768` | Tunnel relay buffer size in bytes (1024–1048576) |
| `max_connections` | `1024` | Max concurrent connections |
| `max_connections_per_ip` | `0` | Concurrent connections per client IP; `0` = unlimited |
| `connect_timeout` | `10` | Total budget (DNS + connect) when reaching a target, in seconds |
| `handshake_timeout` | `10` | Timeout for the client handshake / HTTP request head (seconds) |
| `tcp_keepalive` | `60` | TCP keepalive idle seconds; detects abandoned peers. `0` = off |
| `dns_cache_ttl` | `60` | DNS cache TTL in seconds; `0` = off |
| `auth_cache_ttl` | `60` | Cache TTL for successful credential checks in seconds; `0` = off |
| `max_auth_concurrency` | `0` | Max concurrent bcrypt verifications; `0` = number of CPUs |
| `upstream_keepalive` | `true` | Reuse upstream HTTP connections across client connections |
| `upstream_pool_size` | `8` | Idle upstream connections kept per host; `0` = pooling off |
| `upstream_idle_timeout` | `30` | Seconds an idle upstream connection is kept (seconds) |
| `metrics_address` | `""` (off) | Metrics/health endpoint address, e.g. `127.0.0.1:1081` |
| `log.non_blocking` | `true` | Write log records on a dedicated thread (bounded queue) |
| `log.format` | `{d(%Y-%m-%d %H:%M:%S%.3f)} {l:5} {t} - {m}` | chrono-style pattern (level, target, message); a trailing newline is added when missing |
| `log.queue_capacity` | `8192` | Async writer queue length (64–1048576) |
| `log.warn_rate_limit` | `5` | Max warning records per category per second; `0` = unlimited |

The configuration is validated at startup (address format, buffer size, log
level, archive pattern, empty credentials…). Invalid values abort startup with a
clear error message instead of silently falling back to defaults.

## Logging

- **Asynchronous by default**: records are handed to a bounded queue and written
  by a dedicated thread per sink, so a slow disk never blocks a Tokio worker.
  Set `log.non_blocking = false` for strictly synchronous writes.
- **Loss is never silent**: when the queue is full records are dropped, counted in
  `rust_proxy_log_dropped_total`, and reported with a throttled warning; failed
  writes are counted in `rust_proxy_log_write_errors_total`.
- **Rate limited warnings**: connection errors and authentication failures are
  remote-input driven, so each category emits at most `warn_rate_limit` records
  per second; suppressed records are reported as `(N similar suppressed)`.
- **Rotation**: size based (`log.file_size` MB, `log.file_count` archives, pattern
  `log.archive_pattern`); on unix the active file is created with `0600`.
- **Diagnosable lines**: millisecond timestamps, padded level and the module
  target are part of the default format.

```text
2026-11-12 11:17:36.637 WARN  rust_proxy::proxy::tcp - Connection error from 127.0.0.1:49522: HTTP proxy error: Invalid HTTP request: malformed HTTP head: invalid token
```

## Metrics & Health

Set `metrics_address` (for example `127.0.0.1:1081`) to expose a small HTTP
endpoint:

```bash
curl http://127.0.0.1:1081/healthz      # -> ok
curl http://127.0.0.1:1081/metrics      # Prometheus text format
```

Exported metrics:

| Metric | Type | Meaning |
|--------|------|---------|
| `rust_proxy_connections_total` | counter | Accepted client connections |
| `rust_proxy_connections_active` | gauge | Currently active client connections |
| `rust_proxy_connections_rejected_limit_total` | counter | Rejected by the global limit |
| `rust_proxy_connections_rejected_ip_total` | counter | Rejected by the per-IP limit |
| `rust_proxy_auth_failures_total` | counter | Failed authentication attempts |
| `rust_proxy_upstream_errors_total` | counter | Upstream connect / relay failures |
| `rust_proxy_http_requests_total` | counter | Forwarded HTTP requests |
| `rust_proxy_socks5_connects_total` | counter | Established SOCKS5 CONNECT tunnels |
| `rust_proxy_pool_hits_total` | counter | Upstream connections reused |
| `rust_proxy_pool_misses_total` | counter | Requests needing a new upstream connection |
| `rust_proxy_bytes_to_target_total` | counter | Bytes relayed to targets |
| `rust_proxy_bytes_to_client_total` | counter | Bytes relayed to clients |
| `rust_proxy_log_dropped_total` | counter | Log records dropped because the async queue was full |
| `rust_proxy_log_write_errors_total` | counter | Log records that could not be written (e.g. disk full) |

The endpoint is disabled by default and is a plain HTTP/1.1 responder (no
keep-alive, one request per connection), so bind it to a trusted interface.

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
FROM rust:1.88-alpine AS builder
WORKDIR /app
COPY . .
RUN apk add --no-cache musl-dev && cargo build --release

FROM alpine:latest
RUN apk add --no-cache ca-certificates
WORKDIR /app
COPY --from=builder /app/target/release/rust-proxy .
# The shipped config.toml has no users (auth disabled) and listens on localhost.
# Mount your own config at runtime instead of baking credentials into the image.
COPY config.toml config.toml
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
│   ├── main.rs              # Entry point, CLI args, metrics wiring
│   ├── bin/
│   │   └── test_socks5.rs   # Standalone SOCKS5 handshake smoke test
│   ├── common/
│   │   ├── mod.rs
│   │   ├── auth.rs          # bcrypt hashing, concurrency cap, credential cache
│   │   ├── config.rs        # TOML config parsing and validation
│   │   ├── logger.rs        # log4rs setup with rolling file appender
│   │   ├── log_writer.rs    # Non-blocking log writer thread (bounded queue)
│   │   └── metrics.rs       # Counters, /metrics and /healthz endpoint
│   ├── net/
│   │   ├── mod.rs
│   │   └── conn.rs          # BufferedConnection, socket tuning, split halves
│   └── proxy/
│       ├── mod.rs
│       ├── options.rs       # Runtime tuning shared by both protocols
│       ├── tcp.rs           # Listener, per-IP limits, accept backoff, detection
│       ├── socks5.rs        # SOCKS5 protocol (RFC 1928 / RFC 1929)
│       ├── http.rs          # HTTP CONNECT tunnel and plain HTTP forwarding
│       ├── http1.rs         # httparse heads, response framing, body relay
│       ├── pool.rs          # Upstream keep-alive connection pool
│       └── forward.rs       # Upstream connect (DNS cache, budget) and relay
├── config.example.toml
├── config.toml
├── Cargo.toml
├── Cargo.lock
├── LICENSE
├── README.md
├── README_zh-CN.md
└── refactor.md              # Performance/stability design notes
```

`refactor.md` records the performance and stability work: what changed, why,
the risks considered, and what is deliberately deferred.

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
| Request targets | Absolute-form (`GET http://host/path`) and origin-form (`GET /path` + `Host`) |
| Bodies | Streamed, never buffered: `Content-Length` and `Transfer-Encoding: chunked` (passed through verbatim) |
| Auth | `Proxy-Authorization: Basic` with proper `407` responses |
| Errors | `400`, `407`, `408`, `431`, `501`, `502`, `504` responses instead of silent disconnects |

For non-CONNECT requests, headers are forwarded preserving original order and case while stripping hop-by-hop and `Proxy-*` headers. The request body is streamed straight through (constant memory, any size) while the response is relayed back concurrently, so an upstream that answers before consuming the body (413, 401, redirects) can never stall the proxy. Requests with `Expect: 100-continue` are acknowledged before the body is read.

**Upstream keep-alive**: response framing (`Content-Length`, `chunked`, `HEAD`/`204`/`304`, `Connection: close`) is parsed so the proxy knows exactly where a response ends. Fully delimited connections are returned to a per-host pool (`upstream_pool_size`, `upstream_idle_timeout`) and reused by later client connections — no repeated DNS lookup or TCP handshake. Idle connections are probed before reuse, and a stale pooled connection is transparently replaced by a fresh one. The client side always receives `Connection: close` (client keep-alive is not implemented), and hop-by-hop headers are stripped from the response.

A request that carries both `Content-Length` and `Transfer-Encoding: chunked` is rejected with `400` (request smuggling). The request head is capped at 64 KiB / 100 headers, request lines at 8 KiB, and the head must arrive within `handshake_timeout`.

## Security Considerations

1. **Passwords** are bcrypt-hashed at startup — plaintext is never stored in memory after init, and credentials are never written to the logs (only a redacted configuration summary is logged)
2. **Default bind** is `127.0.0.1` (localhost only); use `0.0.0.0` with caution
3. **No TLS** — the proxy does not encrypt traffic itself; rely on HTTPS at the application layer or wrap with a VPN / SSH tunnel
4. **Connection limits** prevent resource exhaustion; tune `max_connections`, `max_connections_per_ip` and `LimitNOFILE` for production
5. **Slow-client protection** — `handshake_timeout` bounds method negotiation and request head parsing; bodies are streamed so they never accumulate in memory
6. **Size limits** — request lines (8 KiB) and headers (64 KiB / 100 fields) are capped to prevent memory exhaustion
7. **Keepalive** — abandoned peers are detected via `tcp_keepalive`, so their sockets and connection slots are reclaimed
8. **Credential cache** — successful checks are cached for `auth_cache_ttl`; only a SHA-256 digest is kept in memory, never plaintext, and it cannot be used to log in

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
| [socket2](https://crates.io/crates/socket2) | TCP keepalive / socket tuning |
| [sha2](https://crates.io/crates/sha2) | SHA-256 digests for the credential cache key |
| [httparse](https://crates.io/crates/httparse) | Zero-copy HTTP/1.x head parsing |
| [anyhow](https://crates.io/crates/anyhow) | Error type required by the log4rs appender trait |

## Performance Tips

1. Tune `relay_buffer_size` (e.g. `65536`) for tunnel throughput; `buffer_size` only affects handshake parsing
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