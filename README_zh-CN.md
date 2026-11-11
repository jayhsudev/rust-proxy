# Rust Proxy

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)
[![CI](https://github.com/jayhsudev/rust-proxy/actions/workflows/rust.yml/badge.svg)](https://github.com/jayhsudev/rust-proxy/actions/workflows/rust.yml)

[English](README.md) | 中文版本

一个高性能、轻量级的异步代理服务器，支持 SOCKS5 和 HTTP 代理协议，使用 Rust 编写。设计理念注重简洁、安全和性能。

## 功能特点

- 🌐 **多协议支持**：SOCKS5（RFC 1928）和 HTTP/HTTPS CONNECT 代理
- 🔍 **自动协议检测**：通过首字节自动识别 SOCKS5 或 HTTP 协议
- 🔐 **用户认证**：bcrypt 密码哈希，支持 SOCKS5（RFC 1929）和 HTTP Basic 认证
- 🚀 **异步 I/O**：基于 Tokio，零拷贝双向数据转发
- 📝 **高度可配置**：TOML 配置文件，所有选项均可通过 CLI 覆盖
- 📋 **滚动日志**：log4rs 按大小自动轮转归档
- 🚦 **连接限制**：基于信号量的并发控制，可配置超时
- 🛡 **安全加固**：握手超时、请求头大小上限、请求体流式转发（内存恒定）、优雅关闭
- ♻️ **上游 keep-alive**：按响应定界解析 + 按 host 维护连接池复用
- 📊 **可观测**：可选 Prometheus `/metrics` 与 `/healthz` 端点
- 🧪 **自动化测试**：单元测试与协议级测试（SOCKS5、HTTP CONNECT/转发/认证/限制）

## 快速开始

```bash
# 1. 克隆并构建
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy
cargo build --release

# 2. 复制示例配置文件
cp config.example.toml config.toml

# 3. 运行
./target/release/rust-proxy

# 4. 测试（默认：127.0.0.1:1080）
curl -x socks5://127.0.0.1:1080 https://httpbin.org/ip
curl -x http://127.0.0.1:1080 https://httpbin.org/ip
```

## 安装

### 前提条件

- Rust 1.88+（2024 edition）
- Cargo

### 从源代码构建

```bash
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy
cargo build --release
./target/release/rust-proxy
```

### 运行测试

```bash
cargo test
cargo test -- --nocapture
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

测试全部在本地回环网络上运行（无需外网），覆盖：配置解析与校验、bcrypt 认证（明文与预哈希密码、并发上限、凭据缓存）、缓冲连接分帧与拆分、SOCKS5 协商/认证/应答码、HTTP CONNECT 隧道、流式与 chunked 请求体、上游提前响应、请求走私拒绝、relay 半关闭传播、per-IP 限额、DNS 缓存、HTTP/1.1 响应定界（Content-Length/chunked/HEAD/204/中间 1xx）、上游连接池复用、指标端点与异步日志 writer。

## 使用方法

### 命令行选项

所有 CLI 参数都会覆盖配置文件中的对应值。

```bash
./rust-proxy                                    # 使用默认 config.toml
./rust-proxy --config path/to/config.toml       # 自定义配置文件
./rust-proxy --listen-address 0.0.0.0:8080      # 覆盖监听地址
./rust-proxy --log-level debug                  # trace, debug, info, warn, error
./rust-proxy --buffer-size 8192                 # 网络缓冲区大小（字节）
./rust-proxy --max-connections 2048             # 最大并发连接数
./rust-proxy --connect-timeout 15               # 目标服务器连接超时（秒）
./rust-proxy --handshake-timeout 5              # 客户端握手超时（秒）
./rust-proxy --help
./rust-proxy --version
```

### 配置文件

创建 `config.toml`（或从 `config.example.toml` 复制）：

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

# 可选 — 移除此部分则无需认证
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

> **注意 TOML 顺序**：顶层配置项必须写在第一个 `[section]` 之前。写在 `[users]`
> 或 `[log]` 之后的键会被解析为对应 section 的成员 —— 例如把 `buffer_size`
> 写在 `[users]` 下，会被当成一个用户账号，从而在不知情的情况下开启认证。

### 配置参考

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `listen_address` | `127.0.0.1:1080` | 监听地址和端口 |
| `users` | `{}`（空） | 用户名/密码对，为空则不启用认证；以 `$2` 开头的值视为已哈希的 bcrypt 密码 |
| `log.level` | `Info` | Off, Error, Warn, Info, Debug, Trace |
| `log.path` | `logs/rust-proxy.log` | 日志文件路径 |
| `log.archive_pattern` | `logs/archive/rust-proxy-{}.log` | 归档文件名模式（必须包含 `{}`） |
| `log.file_count` | `5` | 保留的归档日志文件数量 |
| `log.file_size` | `10` | 单个日志文件最大大小（MB） |
| `buffer_size` | `4096` | 握手解析缓冲区大小（1–65536 字节） |
| `relay_buffer_size` | `32768` | 隧道转发缓冲区大小（1024–1048576 字节） |
| `max_connections` | `1024` | 最大并发连接数 |
| `max_connections_per_ip` | `0` | 单 IP 并发连接上限，`0` = 不限制 |
| `connect_timeout` | `10` | 连接目标服务器的总预算（DNS + 建连，秒） |
| `handshake_timeout` | `10` | 客户端握手 / HTTP 请求头读取超时（秒） |
| `tcp_keepalive` | `60` | TCP keepalive 空闲秒数，用于发现掉线对端；`0` = 关闭 |
| `dns_cache_ttl` | `60` | DNS 缓存秒数；`0` = 关闭 |
| `auth_cache_ttl` | `60` | 认证成功结果缓存秒数；`0` = 关闭 |
| `max_auth_concurrency` | `0` | 并发 bcrypt 校验上限；`0` = CPU 核数 |
| `upstream_keepalive` | `true` | 跨客户端连接复用上游 HTTP 连接 |
| `upstream_pool_size` | `8` | 每个 host 保留的空闲连接数；`0` = 关闭连接池 |
| `upstream_idle_timeout` | `30` | 空闲上游连接保留时长（秒） |
| `metrics_address` | `""`（关闭） | 指标/健康检查监听地址，如 `127.0.0.1:1081` |
| `log.non_blocking` | `true` | 日志写盘放到独立线程（有界队列） |

启动时会校验配置（监听地址格式、缓冲区大小、日志级别、归档模式、空密码等），
非法值会直接报错退出，而不是静默回退到默认值。

## 指标与健康检查

配置 `metrics_address`（例如 `127.0.0.1:1081`）即可开启一个小型 HTTP 端点：

```bash
curl http://127.0.0.1:1081/healthz      # -> ok
curl http://127.0.0.1:1081/metrics      # Prometheus 文本格式
```

导出的指标：

| 指标 | 类型 | 含义 |
|------|------|------|
| `rust_proxy_connections_total` | counter | 已接受的客户端连接数 |
| `rust_proxy_connections_active` | gauge | 当前活跃客户端连接数 |
| `rust_proxy_connections_rejected_limit_total` | counter | 被全局并发限制拒绝的连接 |
| `rust_proxy_connections_rejected_ip_total` | counter | 被单 IP 限制拒绝的连接 |
| `rust_proxy_auth_failures_total` | counter | 认证失败次数 |
| `rust_proxy_upstream_errors_total` | counter | 上游连接/转发失败次数 |
| `rust_proxy_http_requests_total` | counter | 已转发的 HTTP 请求数 |
| `rust_proxy_socks5_connects_total` | counter | 已建立的 SOCKS5 CONNECT 隧道 |
| `rust_proxy_pool_hits_total` | counter | 命中连接池复用的次数 |
| `rust_proxy_pool_misses_total` | counter | 需要新建上游连接的次数 |
| `rust_proxy_bytes_to_target_total` | counter | 转发到目标端的字节数 |
| `rust_proxy_bytes_to_client_total` | counter | 转发到客户端的字节数 |

该端点默认关闭，且是简化的 HTTP/1.1 实现（不支持 keep-alive，一次请求一条连接），请只绑定到可信接口。

## 客户端配置

### curl

```bash
# SOCKS5
curl -x socks5://127.0.0.1:1080 https://httpbin.org/ip

# SOCKS5 带认证
curl -x socks5://alice:password123@127.0.0.1:1080 https://httpbin.org/ip

# HTTP
curl -x http://127.0.0.1:1080 https://httpbin.org/ip

# HTTP 带认证
curl -x http://alice:password123@127.0.0.1:1080 https://httpbin.org/ip
```

### 环境变量

```bash
export http_proxy=http://127.0.0.1:1080
export https_proxy=http://127.0.0.1:1080
export ALL_PROXY=socks5://127.0.0.1:1080
```

### 浏览器

**Firefox**：设置 → 网络设置 → 手动代理配置 → SOCKS 主机 `127.0.0.1`，端口 `1080`，选择 SOCKS v5，勾选"使用 SOCKS v5 时代理 DNS 查询"。

**Chrome**：
```bash
google-chrome --proxy-server="socks5://127.0.0.1:1080"
google-chrome --proxy-server="http://127.0.0.1:1080"
```

### Git

```bash
git config --global http.proxy socks5://127.0.0.1:1080
git config --global https.proxy socks5://127.0.0.1:1080

# 移除代理
git config --global --unset http.proxy
git config --global --unset https.proxy
```

## 部署

### systemd（Linux）

创建 `/etc/systemd/system/rust-proxy.service`：

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
# 仓库自带 config.toml 不含用户（认证关闭）且仅监听本地；
# 请在运行时挂载自己的配置，不要把凭据打进镜像。
COPY config.toml config.toml
EXPOSE 1080
CMD ["./rust-proxy"]
```

```bash
docker build -t rust-proxy .
docker run -d -p 1080:1080 -v $(pwd)/config.toml:/app/config.toml rust-proxy
```

### 后台运行（Unix）

```bash
nohup ./rust-proxy > proxy.out 2>&1 &
pkill rust-proxy   # 停止
```

## 项目结构

```
rust-proxy/
├── src/
│   ├── main.rs              # 入口，CLI 参数，指标端点装配
│   ├── bin/
│   │   └── test_socks5.rs   # SOCKS5 握手冒烟测试
│   ├── common/
│   │   ├── mod.rs
│   │   ├── auth.rs          # bcrypt 哈希、并发上限、凭据缓存
│   │   ├── config.rs        # TOML 配置解析与校验
│   │   ├── logger.rs        # log4rs 滚动文件日志
│   │   ├── log_writer.rs    # 异步日志写入线程（有界队列）
│   │   └── metrics.rs       # 计数器 与 /metrics、/healthz 端点
│   ├── net/
│   │   ├── mod.rs
│   │   └── conn.rs          # BufferedConnection、socket 调优、拆分为读写半部
│   └── proxy/
│       ├── mod.rs
│       ├── options.rs       # 两种协议共享的运行参数
│       ├── tcp.rs           # 监听、per-IP 限额、accept 退避、协议识别
│       ├── socks5.rs        # SOCKS5 协议（RFC 1928 / RFC 1929）
│       ├── http.rs          # HTTP CONNECT 隧道与普通 HTTP 转发
│       ├── http1.rs         # httparse 头解析、响应定界、body 转发
│       ├── pool.rs          # 上游 keep-alive 连接池
│       └── forward.rs       # 上游连接（DNS 缓存、总预算）与双向转发
├── config.example.toml
├── config.toml
├── Cargo.toml
├── Cargo.lock
├── LICENSE
├── README.md
├── README_zh-CN.md
└── refactor.md              # 性能与稳定性设计记录
```

## 协议支持

### SOCKS5（RFC 1928）

| 特性 | 详情 |
|------|------|
| 命令 | CONNECT (`0x01`) |
| 地址类型 | IPv4 (`0x01`)、域名 (`0x03`)、IPv6 (`0x04`) |
| 认证方式 | 无认证 (`0x00`)、用户名/密码 (`0x02`, RFC 1929) |

未配置用户时，服务端也接受仅提供方法 `0x02` 的客户端 — 认证阶段自动放行。

### HTTP 代理

| 特性 | 详情 |
|------|------|
| CONNECT | 通过双向转发实现 HTTPS 隧道 |
| GET / POST / PUT / DELETE / HEAD / OPTIONS / PATCH | 普通 HTTP 转发，自动剥离逐跳代理头 |
| 请求目标 | 绝对 URI（`GET http://host/path`）与 origin-form（`GET /path` + `Host`） |
| 请求体 | 全程流式转发、不缓冲：`Content-Length` 与 `Transfer-Encoding: chunked`（原样透传） |
| 认证 | `Proxy-Authorization: Basic`，正确返回 `407` 响应 |
| 错误响应 | 返回 `400`、`407`、`408`、`431`、`501`、`502`、`504`，不再静默断开 |

非 CONNECT 请求转发时保留原始 header 顺序和大小写，剥离逐跳头与 `Proxy-*` 头。请求体边读边转发（内存恒定、大小不受限），同时并发回传响应，因此上游"未读完请求体就先响应"（413/401/重定向）也不会造成死锁。带 `Expect: 100-continue` 的请求会先回 `100 Continue` 再读取请求体。

**上游 keep-alive**：解析响应定界（`Content-Length`、`chunked`、`HEAD`/`204`/`304`、`Connection: close`）以确定响应结束位置；可完整定界的连接会放回按 host 维护的连接池（`upstream_pool_size`、`upstream_idle_timeout`），供后续客户端连接复用，省去重复 DNS 解析与 TCP 握手。复用前会探测空闲连接是否存活，失效的连接会自动用新连接替换。客户端侧始终收到 `Connection: close`（未实现客户端 keep-alive），响应中的逐跳头会被剥离。

同时携带 `Content-Length` 与 `Transfer-Encoding: chunked` 的请求会被直接拒绝（`400`，防请求走私）。请求头总大小（64 KiB / 100 个字段）、请求行（8 KiB）均有上限，且必须在 `handshake_timeout` 内完成。

## 安全注意事项

1. **密码** 在启动时进行 bcrypt 哈希 — 初始化后内存中不保留明文；凭据不会写入日志（仅输出脱敏后的配置摘要）
2. **默认绑定** `127.0.0.1`（仅本地）；使用 `0.0.0.0` 请谨慎
3. **无 TLS** — 代理本身不加密流量，请在应用层使用 HTTPS 或通过 VPN / SSH 隧道保护传输
4. **连接限制** 防止资源耗尽；生产环境请调整 `max_connections` 和 `LimitNOFILE`
5. **慢速攻击防护** — `handshake_timeout` 约束方法协商与请求头解析；请求体流式转发，不在内存中累积
6. **大小限制** — 请求行（8 KiB）、请求头（64 KiB / 100 个字段）均有上限，防止内存耗尽
7. **保活探测** — `tcp_keepalive` 让掉线对端的 socket 与并发许可被及时回收
8. **凭据缓存** — 成功认证结果按 `auth_cache_ttl` 缓存，内存中只保存 SHA-256 摘要（非明文，且无法用于登录）

## 依赖项

| 库 | 用途 |
|----|------|
| [tokio](https://crates.io/crates/tokio) | 异步运行时 |
| [clap](https://crates.io/crates/clap) | 命令行参数解析 |
| [serde](https://crates.io/crates/serde) | 序列化 / 反序列化 |
| [config](https://crates.io/crates/config) | 配置文件处理 |
| [log](https://crates.io/crates/log) | 日志门面 |
| [log4rs](https://crates.io/crates/log4rs) | 滚动文件日志 |
| [thiserror](https://crates.io/crates/thiserror) | 错误类型定义 |
| [bcrypt](https://crates.io/crates/bcrypt) | 密码哈希 |
| [base64](https://crates.io/crates/base64) | Base64 编解码 |
| [url](https://crates.io/crates/url) | URL 解析 |
| [socket2](https://crates.io/crates/socket2) | TCP keepalive / socket 调优 |
| [sha2](https://crates.io/crates/sha2) | 认证缓存键的 SHA-256 摘要 |
| [httparse](https://crates.io/crates/httparse) | 零拷贝 HTTP/1.x 请求/响应头解析 |
| [anyhow](https://crates.io/crates/anyhow) | log4rs appender trait 要求的错误类型 |

## 性能建议

1. 高吞吐场景下调整 `relay_buffer_size`（如 `65536`）；`buffer_size` 只影响握手解析
2. 大量并发连接时提升系统文件描述符限制（`ulimit -n`）
3. 生产环境务必使用 `cargo build --release` 构建
4. 生产环境使用 `Warn` 或 `Info` 日志级别 — `Debug` / `Trace` 会带来明显开销

## 故障排除

**端口被占用**
```bash
lsof -i :1080
ss -tlnp | grep 1080
```

**连接被拒绝** — 确认代理正在运行、监听地址与客户端配置一致、防火墙放行对应端口。

**认证失败** — 用户名/密码区分大小写；确认 `config.toml` 中存在 `[users]` 部分且客户端发送了认证信息。

**调试模式**
```bash
./rust-proxy --log-level debug
tail -f logs/rust-proxy.log
```

## 贡献

1. Fork 本仓库
2. 创建功能分支（`git checkout -b feature/amazing-feature`）
3. 提交更改（`git commit -m 'Add amazing feature'`）
4. 推送（`git push origin feature/amazing-feature`）
5. 发起 Pull Request

## 许可证

MIT — 详情见 [LICENSE](LICENSE) 文件。