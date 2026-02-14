# Rust Proxy

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2021-orange.svg)](https://www.rust-lang.org)
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

- Rust 1.70+（2021 edition）
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
cargo clippy
```

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
./rust-proxy --help
./rust-proxy --version
```

### 配置文件

创建 `config.toml`（或从 `config.example.toml` 复制）：

```toml
listen_address = "127.0.0.1:1080"

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

buffer_size = 4096
max_connections = 1024
connect_timeout = 10
```

### 配置参考

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `listen_address` | `127.0.0.1:1080` | 监听地址和端口 |
| `users` | `{}`（空） | 用户名/密码对，为空则不启用认证 |
| `log.level` | `Info` | Off, Error, Warn, Info, Debug, Trace |
| `log.path` | `logs/rust-proxy.log` | 日志文件路径 |
| `log.archive_pattern` | `logs/archive/rust-proxy-{}.log` | 归档文件名模式（`{}` = 序号） |
| `log.file_count` | `5` | 保留的归档日志文件数量 |
| `log.file_size` | `10` | 单个日志文件最大大小（MB） |
| `buffer_size` | `4096` | 网络缓冲区大小（1–65536 字节） |
| `max_connections` | `1024` | 最大并发连接数 |
| `connect_timeout` | `10` | 连接目标服务器的超时时间（秒） |

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

### 后台运行（Unix）

```bash
nohup ./rust-proxy > proxy.out 2>&1 &
pkill rust-proxy   # 停止
```

## 项目结构

```
rust-proxy/
├── src/
│   ├── main.rs              # 入口，CLI 参数，备用 logger
│   ├── bin/
│   │   └── test_socks5.rs   # SOCKS5 握手冒烟测试
│   ├── common/
│   │   ├── mod.rs
│   │   ├── auth.rs          # bcrypt 密码哈希与验证
│   │   ├── config.rs        # TOML 配置解析与校验
│   │   └── logger.rs        # log4rs 滚动文件日志
│   ├── net/
│   │   ├── mod.rs
│   │   └── conn.rs          # BufferedConnection（AsyncRead/AsyncWrite）
│   └── proxy/
│       ├── mod.rs
│       ├── tcp.rs            # 监听、协议检测、并发控制
│       ├── socks5.rs         # SOCKS5 协议（RFC 1928 / RFC 1929）
│       ├── http.rs           # HTTP CONNECT 隧道与普通 HTTP 转发
│       └── forward.rs        # 地址解析、超时连接、双向拷贝
├── config.example.toml
├── config.toml
├── Cargo.toml
├── Cargo.lock
├── LICENSE
├── README.md
└── README_zh-CN.md
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
| 认证 | `Proxy-Authorization: Basic`，正确返回 `407` 响应 |

非 CONNECT 请求转发时保留原始 header 顺序和大小写，注入 `Connection: close`，响应单向拷贝（目标 → 客户端）。

## 安全注意事项

1. **密码** 在启动时进行 bcrypt 哈希 — 初始化后内存中不保留明文
2. **默认绑定** `127.0.0.1`（仅本地）；使用 `0.0.0.0` 请谨慎
3. **无 TLS** — 代理本身不加密流量，请在应用层使用 HTTPS 或通过 VPN / SSH 隧道保护传输
4. **连接限制** 防止资源耗尽；生产环境请调整 `max_connections` 和 `LimitNOFILE`

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

## 性能建议

1. 高吞吐场景下增大 `buffer_size`（如 `16384`）
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