# Rust Proxy

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2021-orange.svg)](https://www.rust-lang.org)
[![CI](https://github.com/jayhsudev/rust-proxy/actions/workflows/rust.yml/badge.svg)](https://github.com/jayhsudev/rust-proxy/actions/workflows/rust.yml)

[English](README.md) | 中文版本

一个高性能、轻量级的异步代理服务器，支持SOCKS5、HTTP和TCP协议，使用Rust编写。设计理念注重简洁、安全和性能。

## 功能特点

- 🔌 **多协议支持**：SOCKS5 (v5)、HTTP和HTTPS CONNECT代理协议
- 🔒 **用户认证**：使用bcrypt密码哈希的安全用户验证机制
- 🔧 **高度可配置**：监听地址、日志级别、缓冲区大小等多项配置
- 📝 **TOML配置**：易于使用的配置文件格式
- 🚀 **高性能**：基于Tokio运行时的异步设计
- 📊 **高级日志系统**：使用log4rs的全面日志记录功能，支持文件轮转
- 💾 **内存高效**：可配置的缓冲区大小和连接处理
- 🔄 **自动协议检测**：自动识别SOCKS5或HTTP协议

## 快速开始

```bash
# 1. 克隆并构建
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy
cargo build --release

# 2. 复制示例配置文件（必需）
cp config.example.toml config.toml

# 3. 运行代理服务器
./target/release/rust-proxy

# 4. 使用代理（默认：localhost:1080）
# 将您的应用程序配置为使用 SOCKS5 代理 127.0.0.1:1080
```

## 安装

### 前提条件

- Rust 1.70或更高版本（2021版本）
- Cargo（Rust包管理器）

### 从源代码构建

```bash
# 克隆仓库
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy

# 使用发布模式构建项目（优化性能）
cargo build --release

# 二进制文件将位于target/release/目录下
./target/release/rust-proxy
```

### 运行测试

```bash
# 运行所有测试
cargo test

# 运行测试并显示输出
cargo test -- --nocapture

# 运行clippy进行代码检查
cargo clippy
```

## 使用方法

### 命令行选项

```bash
# 使用默认配置运行
./rust-proxy

# 指定自定义配置文件
./rust-proxy --config path/to/config.toml

# 指定监听地址（覆盖配置文件）
./rust-proxy --listen-address 127.0.0.1:1080

# 设置日志级别（trace, debug, info, warn, error）
./rust-proxy --log-level debug

# 设置缓冲区大小（字节，覆盖配置文件）
./rust-proxy --buffer-size 8192

# 显示帮助
./rust-proxy --help

# 显示版本
./rust-proxy --version
```

### 配置文件

创建一个`config.toml`文件（或从`config.example.toml`复制）：

```toml
# 代理服务器将监听的地址和端口
listen_address = "127.0.0.1:1080"

# 认证用户（可选，移除此部分则无需认证）
[users]
alice = "password123"
bob = "securepass"

# 日志配置
[log]
level = "Info"                                    # Off, Error, Warn, Info, Debug, Trace
path = "logs/rust-proxy.log"                      # 日志文件路径
archive_pattern = "logs/archive/rust-proxy-{}.log" # 归档模式
file_count = 5                                    # 保留的日志文件数量
file_size = 10                                    # 最大文件大小（MB）

# 网络操作的缓冲区大小（字节）
buffer_size = 4096
```

### 配置选项

| 选项 | 默认值 | 描述 |
|------|--------|------|
| `listen_address` | `127.0.0.1:1080` | 监听地址和端口 |
| `users` | `{}` (空) | 用于认证的用户名/密码对 |
| `log.level` | `Info` | 日志级别：Off, Error, Warn, Info, Debug, Trace |
| `log.path` | `logs/rust-proxy.log` | 日志文件路径 |
| `log.archive_pattern` | `logs/archive/rust-proxy-{}.log` | 归档日志的模式 |
| `log.file_count` | `5` | 保留的归档日志文件数量 |
| `log.file_size` | `10` | 每个日志文件的最大大小（MB） |
| `buffer_size` | `4096` | 网络缓冲区大小（1-65536字节） |

## 客户端配置

### 使用curl

```bash
# SOCKS5代理
curl -x socks5://127.0.0.1:1080 https://httpbin.org/ip

# 带认证的SOCKS5代理
curl -x socks5://alice:password123@127.0.0.1:1080 https://httpbin.org/ip

# HTTP代理
curl -x http://127.0.0.1:1080 https://httpbin.org/ip

# 带认证的HTTP代理
curl -x http://alice:password123@127.0.0.1:1080 https://httpbin.org/ip
```

### 使用wget

```bash
# HTTP代理
https_proxy=http://127.0.0.1:1080 wget https://httpbin.org/ip

# 带认证
https_proxy=http://alice:password123@127.0.0.1:1080 wget https://httpbin.org/ip
```

### 环境变量

设置这些环境变量以在系统范围内使用代理：

```bash
# HTTP/HTTPS代理
export http_proxy=http://127.0.0.1:1080
export https_proxy=http://127.0.0.1:1080

# SOCKS5代理（取决于应用程序支持）
export ALL_PROXY=socks5://127.0.0.1:1080
```

### 浏览器配置

#### Firefox
1. 打开 设置 → 网络设置 → 设置
2. 选择"手动代理配置"
3. 对于SOCKS5：将SOCKS主机设置为`127.0.0.1`，端口设置为`1080`
4. 选择"SOCKS v5"
5. 勾选"使用SOCKS v5时代理DNS查询"

#### Chrome/Chromium
```bash
# 使用SOCKS5代理启动
google-chrome --proxy-server="socks5://127.0.0.1:1080"

# 使用HTTP代理启动
google-chrome --proxy-server="http://127.0.0.1:1080"
```

### Git配置

```bash
# Git的SOCKS5代理
git config --global http.proxy socks5://127.0.0.1:1080
git config --global https.proxy socks5://127.0.0.1:1080

# Git的HTTP代理
git config --global http.proxy http://127.0.0.1:1080
git config --global https.proxy http://127.0.0.1:1080

# 移除代理设置
git config --global --unset http.proxy
git config --global --unset https.proxy
```

## 部署

### 作为systemd服务运行（Linux）

创建`/etc/systemd/system/rust-proxy.service`：

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

然后启用并启动服务：

```bash
sudo systemctl daemon-reload
sudo systemctl enable rust-proxy
sudo systemctl start rust-proxy
sudo systemctl status rust-proxy
```

### 使用Docker运行

创建`Dockerfile`：

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

构建并运行：

```bash
docker build -t rust-proxy .
docker run -d -p 1080:1080 -v $(pwd)/config.toml:/app/config.toml rust-proxy
```

### 后台运行（Unix）

```bash
# 使用nohup
nohup ./rust-proxy > proxy.out 2>&1 &

# 检查是否运行
ps aux | grep rust-proxy

# 停止代理
pkill rust-proxy
```

## 项目结构

```
rust-proxy/
├── src/
│   ├── main.rs           # 应用程序入口点和命令行参数处理
│   ├── common/           # 通用工具和共享模块
│   │   ├── mod.rs        # 模块声明
│   │   ├── auth.rs       # 使用bcrypt密码哈希的用户认证
│   │   ├── config.rs     # 配置文件解析和验证
│   │   ├── logger.rs     # 使用log4rs的日志设置和文件轮转
│   │   └── utils.rs      # 实用工具函数（base64编码等）
│   ├── net/              # 网络层抽象
│   │   ├── mod.rs        # 网络模块声明
│   │   └── conn.rs       # 带异步I/O的缓冲连接处理
│   └── proxy/            # 代理协议实现
│       ├── mod.rs        # 代理模块声明和导出
│       ├── tcp.rs        # 带自动协议检测的TCP监听器
│       ├── socks5.rs     # SOCKS5代理协议（RFC 1928）
│       ├── http.rs       # HTTP/HTTPS CONNECT代理处理器
│       └── forward.rs    # 双向数据转发
├── .github/workflows/    # GitHub Actions CI/CD配置
│   └── rust.yml          # Rust构建、测试和lint工作流
├── Cargo.toml            # Rust项目清单和依赖项
├── Cargo.lock            # 依赖锁定文件
├── config.example.toml   # 带文档的示例配置
├── LICENSE               # MIT许可证文件
├── README.md             # 英文文档
└── README_zh-CN.md       # 中文文档（本文件）
```

## 协议支持

### SOCKS5 (RFC 1928)

代理实现了SOCKS5协议，具有以下功能：

- **命令**：CONNECT (0x01)
- **地址类型**：IPv4 (0x01)、域名 (0x03)、IPv6 (0x04)
- **认证方法**：
  - 无认证 (0x00) - 当未配置用户时
  - 用户名/密码 (0x02) - RFC 1929

### HTTP代理

代理支持HTTP代理协议：

- **CONNECT方法**：用于HTTPS隧道
- **GET/POST等**：用于普通HTTP请求（转发到目标）
- **Proxy-Authorization**：Basic认证支持

## 安全注意事项

1. **密码存储**：配置中的密码在启动时使用bcrypt进行哈希
2. **认证**：支持SOCKS5和HTTP Basic认证
3. **绑定地址**：默认绑定到`127.0.0.1`（仅本地）
   - 使用`0.0.0.0`接受外部连接（请谨慎使用）
4. **无加密**：代理本身不加密流量
   - 在应用层使用HTTPS/TLS
   - 考虑使用VPN或SSH隧道来保证传输安全

## 故障排除

### 常见问题

**端口已被占用**
```bash
# 查找使用该端口的进程
lsof -i :1080
# 或在Linux上
ss -tlnp | grep 1080
```

**小于1024的端口权限被拒绝**
```bash
# 以root运行（不推荐）或使用>=1024的端口
./rust-proxy --listen-address 0.0.0.0:1080
```

**连接被拒绝**
- 确保代理正在运行：`ps aux | grep rust-proxy`
- 检查监听地址是否与客户端配置匹配
- 验证防火墙规则允许连接

**认证失败**
- 确保用户名/密码完全匹配（区分大小写）
- 检查config.toml中是否存在`[users]`部分
- 对于SOCKS5，确保您的客户端支持认证

### 调试模式

使用调试日志运行以诊断问题：

```bash
./rust-proxy --log-level debug
```

查看日志文件获取详细信息：

```bash
tail -f logs/rust-proxy.log
```

## 依赖项

| 库 | 用途 |
|----|------|
| [tokio](https://crates.io/crates/tokio) | 异步运行时 |
| [clap](https://crates.io/crates/clap) | 命令行参数解析 |
| [serde](https://crates.io/crates/serde) | 序列化/反序列化 |
| [config](https://crates.io/crates/config) | 配置管理 |
| [toml](https://crates.io/crates/toml) | TOML文件解析 |
| [log4rs](https://crates.io/crates/log4rs) | 带文件轮转的日志 |
| [log](https://crates.io/crates/log) | 日志门面 |
| [thiserror](https://crates.io/crates/thiserror) | 错误类型定义 |
| [bcrypt](https://crates.io/crates/bcrypt) | 密码哈希 |
| [base64](https://crates.io/crates/base64) | Base64编码/解码 |
| [url](https://crates.io/crates/url) | URL解析 |

## 性能提示

1. **缓冲区大小**：对于高吞吐量场景，增加`buffer_size`（例如16384）
2. **文件描述符**：对于大量并发连接，增加系统限制
3. **发布构建**：生产环境始终使用`cargo build --release`
4. **日志级别**：生产环境使用`Info`或`Warn`；`Debug`/`Trace`会增加开销

## 贡献

欢迎贡献！请随时提交Pull Request或开启Issue。

1. Fork本仓库
2. 创建您的功能分支（`git checkout -b feature/amazing-feature`）
3. 提交您的更改（`git commit -m '添加一些很棒的功能'`）
4. 推送到分支（`git push origin feature/amazing-feature`）
5. 开启Pull Request

## 许可证

本项目采用MIT许可证 - 详情请查看[LICENSE](LICENSE)文件。

## 鸣谢

- 使用[Rust](https://www.rust-lang.org/)构建 - 一种让每个人都能构建可靠、高效软件的语言
- 感谢所有帮助改进这个项目的贡献者