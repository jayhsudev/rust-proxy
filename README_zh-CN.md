# Rust Proxy

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2021-orange.svg)](https://www.rust-lang.org)
[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)](#)

[English](README.md) | 中文版本

一个高性能、轻量级的异步代理服务器，支持SOCKS5、HTTP和TCP协议，使用Rust编写。设计理念注重简洁、安全和性能。

## 快速开始

```bash
# 1. 克隆并构建
git clone https://github.com/jayhsudev/rust-proxy.git
cd rust-proxy
cargo build --release

# 2. 复制示例配置文件（可选）
cp config.example.toml config.toml

# 3. 运行代理服务器
./target/release/rust-proxy

# 4. 使用代理（默认：localhost:1080）
# 将您的应用程序配置为使用 SOCKS5 代理 127.0.0.1:1080
```

## 功能特点

- 🔌 **多协议支持**：SOCKS5、HTTP和TCP代理协议
- 🔒 **用户认证**：使用bcrypt密码哈希的安全用户验证机制
- 🔧 **高度可配置**：监听地址、日志级别、缓冲区大小等多项配置
- 📝 **TOML配置**：易于使用的配置文件格式
- 🔐 **TLS支持**：基于native-tls的安全连接保障
- 🚀 **高性能**：基于Tokio运行时的异步设计
- 📊 **高级日志系统**：使用log4rs的全面日志记录功能，支持文件轮转
- 💾 **内存高效**：可配置的缓冲区大小和连接处理

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
# 可以通过以下命令运行：
./target/release/rust-proxy
```

## 使用方法

### 命令行选项

```bash
# 使用默认配置运行
./rust-proxy

# 指定自定义配置文件
./rust-proxy --config path/to/config.toml

# 指定监听地址
./rust-proxy --listen-address 127.0.0.1:1080

# 设置日志级别（trace, debug, info, warn, error）
./rust-proxy --log-level debug

# 设置缓冲区大小（字节）
./rust-proxy --buffer-size 8192
```

### 配置文件格式

创建一个`config.toml`文件，格式如下：

```toml
# 代理服务器将监听的地址和端口
listen_address = "127.0.0.1:1080"

# 认证用户（可选，如果不需要可移除）
[users]
username1 = "password1"
username2 = "password2"

# 日志配置
[log]
level = "info"                                    # 日志级别（trace, debug, info, warn, error）
path = "logs/rust-proxy.log"                      # 日志文件路径
archive_pattern = "logs/archive/rust-proxy-{}.log" # 归档模式
file_count = 5                                    # 保留的日志文件数量
file_size = 10                                    # 最大文件大小（MB）

# 网络操作的缓冲区大小（字节）
buffer_size = 4096
```

### 使用场景示例

```bash
# 使用默认配置的基本用法
./rust-proxy

# 自定义配置文件和日志级别
./rust-proxy --config my_config.toml --log-level debug

# 在特定端口上运行并使用更大的缓冲区
./rust-proxy --listen-address 0.0.0.0:3128 --buffer-size 16384

# 在端口1080上运行SOCKS5代理
./rust-proxy --listen-address 127.0.0.1:1080

# 在端口8080上运行HTTP代理
./rust-proxy --listen-address 127.0.0.1:8080
```

## 项目结构

```
rust-proxy/
├── src/
│   ├── main.rs         # 应用程序入口点和命令行参数处理
│   ├── common/         # 通用工具和共享模块
│   │   ├── mod.rs      # 模块声明
│   │   ├── auth.rs     # 用户认证和授权逻辑
│   │   ├── config.rs   # 配置文件解析和验证
│   │   ├── logger.rs   # 使用log4rs的日志设置和配置
│   │   └── utils.rs    # 实用工具函数和辅助器
│   ├── net/            # 网络层抽象
│   │   ├── mod.rs      # 网络模块声明
│   │   └── conn.rs     # 带缓冲的连接处理和工具
│   └── proxy/          # 代理协议实现
│       ├── mod.rs      # 代理模块声明和导出
│       ├── tcp.rs      # 原始TCP代理实现
│       ├── socks5.rs   # SOCKS5代理协议处理器
│       ├── http.rs     # HTTP/HTTPS代理协议处理器
│       └── forward.rs  # 数据转发和隧道逻辑
├── target/             # Cargo构建产物（生成的）
├── .git/               # Git版本控制元数据
├── .gitignore          # Git忽略模式
├── Cargo.toml          # Rust项目清单和依赖项
├── Cargo.lock          # 依赖锁定文件（生成的）
├── config.toml         # 用户配置文件（从示例创建）
├── config.example.toml # 带注释的示例配置
├── LICENSE             # MIT许可证文件
├── README.md           # 英文文档
└── README_zh-CN.md     # 中文文档
```

## 依赖项

- [tokio](https://crates.io/crates/tokio) - Rust异步运行时
- [clap](https://crates.io/crates/clap) - 命令行参数解析，带有美观的输出
- [serde](https://crates.io/crates/serde) - 序列化和反序列化框架
- [config](https://crates.io/crates/config) - 配置管理库
- [log4rs](https://crates.io/crates/log4rs) - 灵活的日志框架，支持文件轮转
- [log](https://crates.io/crates/log) - 轻量级日志门面
- [native-tls](https://crates.io/crates/native-tls) - 原生TLS实现
- [tokio-native-tls](https://crates.io/crates/tokio-native-tls) - native-tls的Tokio集成
- [thiserror](https://crates.io/crates/thiserror) - 自定义错误类型的错误处理
- [bcrypt](https://crates.io/crates/bcrypt) - 密码哈希函数
- [base64](https://crates.io/crates/base64) - Base64编码和解码
- [url](https://crates.io/crates/url) - URL解析和操作

## 许可证

本项目采用MIT许可证 - 详情请查看[LICENSE](LICENSE)文件。

## 贡献

欢迎贡献！请随时提交Pull Request或开启Issue来改进这个项目。

## 鸣谢

- 感谢所有帮助改进这个项目的贡献者
- 使用[Rust](https://www.rust-lang.org/)构建 - 一种运行速度极快、防止段错误并保证线程安全的系统编程语言