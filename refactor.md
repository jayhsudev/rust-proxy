# rust-proxy 性能与稳定性优化方案

> 状态：实施前评审稿（逐项已对照源码复核）
> 目标版本：0.1.x（不改变对外协议行为，除下文显式列出的两处 HTTP 行为变更）
> 基线：`cargo test` 30 passed / `clippy -D warnings` 0 warning / `cargo fmt --check` 通过

---

## 0. 范围界定

**本轮实施（稳定性和内存优先，其次吞吐）**

| 批次 | 内容 | 风险 |
|------|------|------|
| A | TCP keepalive、bcrypt 并发上限、accept 退避与日志限流、连接级日志降级 | 低 |
| B | HTTP 请求体流式转发（含并发双向）、per-IP 连接限额 | 中 |
| C | 自定义 relay（缓冲区可配 + 半关闭）、DNS 缓存 + 连接总预算、认证结果缓存、tokio 特性裁剪 | 中 |
| D | 配置项、文档、CI、测试补齐 | 低 |

**本轮暂缓（附原因，见 §8）**

- 上游 keep-alive 连接池（需要完整的 HTTP 响应定界解析，收益大但风险高，建议单独一轮）
- `httparse` 替换手写解析器（现有解析器已加上限且有测试覆盖，替换收益 < 回归风险）
- 异步日志 appender（log4rs 的 `Append` trait 需要自定义编码器封装，本轮先用"降低日志量"缓解）
- 指标/健康检查端点（依赖运维形态，先留接口）

---

## 1. 现状复核（问题与证据）

| # | 问题 | 证据 | 影响 | 等级 |
|---|------|------|------|------|
| 1 | 隧道无 TCP keepalive / 空闲探测 | `src/proxy/tcp.rs:154` 仅 `set_nodelay(true)` | 客户端异常掉线（休眠/NAT 超时/断网）不发 FIN，连接永久占用 fd 与并发许可，长跑后 `max_connections` 被僵尸连接占满 | **高** |
| 2 | 每连接一次 bcrypt，且无并发上限 | `src/common/auth.rs:75` `spawn_blocking(verify)`；cost 12 ≈ 250–400 ms CPU | 攻击者用 N 个连接即可打满 CPU 与 blocking 线程池；合法用户被拖慢 | **高** |
| 3 | HTTP 请求体整体缓冲进内存 | `src/proxy/http.rs:291-305`（`MAX_BODY_BYTES = 16 MiB`，`:95`） | 最坏 1024 × 16 MiB ≈ 16 GB RSS，易 OOM | **高** |
| 4 | 仅全局并发限制，无 per-IP 限额 | `src/proxy/tcp.rs:83` `try_acquire_owned` | 单 IP 可占满全部许可，对他人形成 DoS | 中 |
| 5 | accept 出错固定 100 ms 重试且每次 warn | `src/proxy/tcp.rs:120` | `EMFILE` 期间日志洪水（每 100 ms 一条），放大故障 | 中 |
| 6 | `buffer_size` 不影响隧道吞吐 | `src/proxy/forward.rs:75` 使用 `copy_bidirectional`（内部固定 8 KiB） | 配置项名不副实，调参无效；大流量下 syscall 次数偏多 | 中 |
| 7 | 每连接 `info!` 日志 + log4rs 同步写盘 | `src/proxy/socks5.rs:109`、`src/proxy/http.rs:370/417`、`src/common/logger.rs:37` | 同步文件 I/O 阻塞 runtime 工作线程，高 QPS 下是隐形瓶颈 | 中 |
| 8 | DNS 每次连接重新解析，多地址串行重试且各自计满超时 | `src/proxy/forward.rs:52-73` | 最坏 N × `connect_timeout`；同域名重复解析 | 中 |
| 9 | 每连接预分配两个缓冲 | `src/net/conn.rs:18-25`（`temp_buffer` + `read_buffer`） | 每连接多占约 8 KiB；握手后 `temp_buffer` 不再使用 | 低 |
| 10 | `tokio` 启用 `full` | `Cargo.toml:22` | 编译时间与产物体积偏大 | 低 |

已复核的**误判项**（不实施）：

- `panic = "abort"`：已在上一轮移除（会让单连接 panic 杀掉整个进程），本次不再引入。
- `BufferedConnection::poll_read` 缓冲区语义：实现正确，`into_split` 时必须保留缓冲，否则残留字节会丢失（见 §5 风险 R1）。

---

## 2. 变更设计

### 2.1 `src/net/conn.rs` — 连接层

1. 新增 `into_split(self) -> (BufferedReadHalf, BufferedWriteHalf)`：
   - 用 `TcpStream::into_split()` 得到 `OwnedReadHalf/OwnedWriteHalf`（无锁、无额外分配，优于 `tokio::io::split`）。
   - `BufferedReadHalf` 持有 `read_buffer + pos` 游标，`AsyncRead` 先耗尽缓冲再读 socket，保证握手残留字节不丢失。
   - `BufferedWriteHalf` 委托 `AsyncWrite`（含 `shutdown`）。
2. 新增 `tune_stream(&TcpStream, keepalive: Option<Duration>)`：`set_nodelay(true)` + `socket2` 设置 `TCP_KEEPALIVE`（`with_time(idle)`，unix 下追加 `interval=15s`、`retries=3`）。**设置失败只记 debug 日志，不影响连接建立**（部分平台不支持）。
3. `temp_buffer` 保持现状（握手期需要），隧道期随 `into_split` 一起释放。

### 2.2 `src/proxy/forward.rs` — 转发层

1. `Upstream`：持有 `ProxyOptions` + DNS 缓存（`tokio::sync::Mutex<HashMap<String, CacheEntry>>`）。
   - `resolve(addr)`：命中缓存直接返回；未命中 `lookup_host` 并写入（TTL 可配，默认 60 s，0 = 关闭）；条目上限 4096，超限先清理过期、再整体清空。
   - `connect(addr)`：解析后**总预算 = `connect_timeout`**，逐地址尝试，每次尝试使用剩余预算；全部失败返回最后一次错误。
   - 连接成功后对目标 socket 应用 `tune_stream`。
2. 自定义 relay 取代 `copy_bidirectional`：
   ```rust
   pub async fn relay(client: BufferedConnection, target: BufferedConnection, buf: usize) -> io::Result<()>
   ```
   - 两端 `into_split()`，`try_join!` 两个单向 `copy_one_way`，缓冲区为 `vec![0u8; relay_buffer_size]`（默认 32 KiB）。
   - 单向循环：`read == 0` → `flush` + `shutdown`（把 FIN 传播给对端）→ 返回累计字节，**对向继续**（保持半关闭语义，等价于 `copy_bidirectional`）。
   - 任一侧出错 → `try_join!` 取消另一侧并返回错误。
3. 新增 `copy_chunked_body(reader, writer)`：chunked **原样透传**（复制 chunk 头、数据、CRLF、结尾 trailer），单行长度受 8 KiB 限制。

### 2.3 `src/common/auth.rs` — 认证层

1. `AuthManager` 增加：
   - `limiter: Arc<Semaphore>`：并发 bcrypt 校验上限（默认 `available_parallelism()`，可配），避免打满 blocking 池。
   - 成功凭证缓存：键为 `SHA-256(username || 0x00 || password)` 摘要（**不存明文、不存可逆信息**），TTL 默认 60 s，上限 4096 条，可配 0 关闭；仅缓存成功结果，失败不缓存（失败仍需付 bcrypt 成本）。
   - 未知用户等时处理：对固定的 dummy bcrypt 哈希执行一次校验，避免通过响应时间枚举用户名。
2. 新增 `AuthOptions { cache_ttl, max_concurrency }`，`AuthManager::new()` 保持原签名（默认选项），新增 `with_options()`。

### 2.4 `src/proxy/tcp.rs` — 接入层

1. 构造 `Arc<ProxyOptions>` / `Arc<Upstream>` 并在连接间共享。
2. 客户端 socket 应用 `tune_stream`。
3. per-IP 限额：`IpLimiter`（`Mutex<HashMap<IpAddr, usize>>`）+ `IpGuard`（Drop 时自减）。超限直接关闭连接，日志限流（首次 warn，之后每 100 次 warn 一条，其余 debug）。**默认 0 = 不限制**，避免误伤 NAT/多终端共享出口。
4. accept 错误：连续错误指数退避（50 ms × 2ⁿ，上限 3.2 s），成功即复位；日志按次数限流。
5. 协议分发改为把 `BufferedConnection` **按值**交给代理（HTTP 流式转发与 relay 都需要所有权）。

### 2.5 `src/proxy/http.rs` — HTTP 代理

1. `parse_request` 拆分为「请求头解析」+「请求体流式转发」，删除 `MAX_BODY_BYTES` 与 `413`。
2. 请求体三种形态：
   - `Content-Length: n` → `take(n)` 流式拷贝；
   - `Transfer-Encoding: chunked` → 原样透传（保留该头部）；
   - 无 → 立即半关闭上游写端。
   - **两者同时出现 → 400**（防请求走私）。
3. 转发流程改为并发双向，避免"上游提前响应、不再读请求体"时死锁：
   ```rust
   tokio::pin!(request_pump, response_pump);
   select! { r = &mut request_pump => r?, r = &mut response_pump => return Ok(()) }
   response_pump.await?;
   ```
   任一侧完成即取消另一侧，客户端总能收到已到达的响应。
4. 连接级日志降到 `debug`（`HTTP {} {}` / `CONNECT tunnel`），错误仍按级别记录。
5. 保留既有能力：绝对 URI + origin-form、`Expect: 100-continue`、hop-by-hop 剥离、IPv6 地址、`4xx/5xx` 错误响应、请求头上限。

### 2.6 `src/proxy/socks5.rs`

- 改为按值接收连接，末尾交给 `forward::relay`。
- 连接级日志降级为 `debug`。
- 其余（RSV 校验、BND.ADDR 回填、应答码、握手超时）不变。

### 2.7 配置与依赖

新增配置项（全部有默认值，向后兼容）：

| 配置项 | 默认 | 说明 |
|--------|------|------|
| `tcp_keepalive` | `60` | TCP keepalive 空闲秒数，0 = 关闭 |
| `relay_buffer_size` | `32768` | 隧道转发缓冲区（1 KiB–1 MiB） |
| `max_connections_per_ip` | `0` | 单 IP 并发连接上限，0 = 不限制 |
| `dns_cache_ttl` | `60` | DNS 缓存秒数，0 = 关闭 |
| `auth_cache_ttl` | `60` | 认证成功结果缓存秒数，0 = 关闭 |
| `max_auth_concurrency` | `0` | 并发 bcrypt 上限，0 = CPU 核数 |

依赖：新增 `socket2`（keepalive）、`sha2`（认证缓存摘要）；`tokio` 特性由 `full` 裁剪为
`rt-multi-thread, macros, net, io-util, time, signal, sync`。

---

## 3. 行为变更（对使用者可见）

1. **HTTP 请求体不再整体缓冲**：移除 16 MiB 上限与 `413`；超大请求体会被流式转发（内存恒定）。
2. **chunked 请求改为透传**：不再解块重组为 `Content-Length`，上游看到的仍是 `Transfer-Encoding: chunked`。
3. 连接建立/隧道成功的 `info` 日志降为 `debug`（默认 `Info` 级别下不再逐连接刷屏）。
4. 新增 6 个配置项；旧配置文件无需修改即可运行。

---

## 4. 逐项风险与规避

| 编号 | 风险 | 规避/验证 |
|------|------|-----------|
| R1 | `into_split` 丢失握手中已读入缓冲的字节（如 CONNECT 后紧跟的首个 TLS 包） | `BufferedReadHalf` 先耗尽 `read_buffer` 再读 socket；`test_async_read_drains_buffer` + CONNECT 隧道测试覆盖 |
| R2 | 自定义 relay 的半关闭语义写错，导致单向隧道挂死 | 单向循环 EOF 时 `shutdown()` 对端写端、对向继续；新增半关闭测试 |
| R3 | HTTP 并发转发用 `select!` 取消请求体泵，可能截断正常请求 | 仅在"响应已完成"时取消；正常路径（请求体先结束）继续等待完整响应；新增"提前响应"测试 |
| R4 | 请求走私（`Content-Length` + `Transfer-Encoding` 并存） | 同时出现直接 `400`，并补测试 |
| R5 | per-IP 限额误伤 NAT | 默认 0（关闭），需显式开启 |
| R6 | DNS 缓存导致解析结果陈旧 | TTL 默认 60 s、可关闭；缓存键含端口 |
| R7 | 认证缓存被当作"密码落内存" | 仅存 SHA-256 摘要 + TTL + 上限；摘要不可用于登录；可关闭 |
| R8 | keepalive 参数在部分平台不支持导致连接失败 | 设置失败仅 debug 日志，连接继续 |
| R9 | 连接所有权重构导致编译错误/测试失效 | 全部为编译期检查；测试同步改造后全量回归 |
| R10 | 裁剪 tokio 特性后运行时能力缺失 | 按实际使用的 API 逐项核对（`spawn_blocking`/signal/unix socket），编译 + 测试 + 实跑验证 |

---

## 5. 验证计划

1. 静态：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`。
2. 单测/协议测试（现有 30 个全部保留，新增约 10 个）：
   - relay 半关闭传播；
   - HTTP 1 MiB 流式请求体完整性；
   - HTTP 上游提前响应不阻塞（`select!` 路径）；
   - chunked 透传字节级一致；
   - `Content-Length` + `Transfer-Encoding` 并存 → 400；
   - per-IP 限额（独立单元测试）；
   - 认证缓存命中/过期、未知用户等时路径；
   - DNS 缓存 TTL 单元测试；
   - 配置新增项校验。
3. 端到端（本地回环，无外网）：HTTP 正向、SOCKS5、CONNECT 隧道、Basic 认证（407/200）各跑一次。
4. 回归关注点：`--log-level` 覆盖、脱敏摘要、优雅关闭、`max_connections` 行为不变。

---

## 6. 回滚策略

- 每批次独立提交，任一批次失败可单独回退。
- 新增行为均有开关或默认关闭（per-IP 限额、DNS/认证缓存、keepalive）。
- HTTP 流式转发是本轮唯一不可逆的行为变更，若需回退只需恢复 `read_body` 与 `Connection: close` 串行拷贝逻辑（单文件）。

---

## 7. 实施顺序

1. `refactor.md`（本文档）→ 2. 连接层（split/keepalive）→ 3. 配置与 `ProxyOptions` → 4. 认证层 →
5. 转发层（Upstream/relay）→ 6. SOCKS5 → 7. HTTP → 8. 接入层（tcp.rs）→ 9. main 装配 →
10. 测试补齐 → 11. 文档/CI 同步 → 12. 全量验证。

---

## 8. 暂缓项与理由（第二轮已全部落地，见 §10）

| 项 | 当时理由 |
|----|----------|
| 上游 keep-alive 连接池 | 需要判定响应结束（`Content-Length` / chunked / `204` / `HEAD` / 中间 `1xx`），错误实现会串包或漏响应；收益大但必须配合完整的 HTTP 帧解析与压力测试，单列一轮 |
| `httparse` 替换手写解析 | 现解析器已限制行长/头数/总大小并有测试；替换属于重构而非修复，收益主要是少量分配 |
| 异步日志 appender | 需自定义 `log4rs::append::Append` 与编码缓冲；本轮先通过"连接级日志降级"把日志量降两个数量级，必要时下一轮再做 |
| 指标/健康检查 | 依赖部署形态（Prometheus / HTTP 管理口），需先确定产品形态 |

---

## 9. 实施结果（已落地）

### 9.1 代码变更

| 文件 | 变更 |
|------|------|
| `src/net/conn.rs` | 新增 `tune_stream()`（nodelay + socket2 keepalive）、`into_split()` 及 `BufferedReadHalf`/`BufferedWriteHalf`（保留握手残留字节、无锁） |
| `src/proxy/options.rs` | 新增 `ProxyOptions`（buffer、relay buffer、超时、keepalive、DNS TTL） |
| `src/proxy/forward.rs` | 新增 `Upstream`（DNS 缓存 + 连接总预算 + 目标侧 keepalive）、`relay()`（可配缓冲、半关闭传播、`try_join!`）、`copy_chunked_body()`（chunked 原样透传） |
| `src/common/auth.rs` | 新增 `AuthOptions`、并发信号量、成功结果缓存（SHA-256 摘要 + TTL + 4096 上限）、未知用户等时校验 |
| `src/common/config.rs` | 新增 6 个配置项与校验、摘要输出更新 |
| `src/proxy/http.rs` | 请求体流式转发（`Content-Length`/chunked）、并发双向转发、走私防护（CL+TE → 400）、连接级日志降级 |
| `src/proxy/socks5.rs` | 按值接收连接交由 `relay()`、连接级日志降级 |
| `src/proxy/tcp.rs` | `IpLimiter`（per-IP 限额 + Drop 归还）、accept 指数退避、拒绝日志限流、客户端 socket 调优 |
| `src/main.rs` | 装配 `ProxyOptions` / `Upstream` / `AuthOptions` |
| `Cargo.toml` | 新增 `socket2`、`sha2`；`tokio` 特性由 `full` 裁剪为 7 项；`rust-version = "1.88"` |

### 9.2 实施过程中发现并修正的问题

1. **`select!` 取消传播错误（真实回归，已修）**：端到端验证 200 KB POST 时发现，当上游提前关闭导致请求体侧写入失败时，`select!` 会把响应方向一并取消，客户端收不到已到达的响应。修正为：请求侧失败只记 debug 日志，继续等待响应方向结束；并且无论成功失败都先对上游半关闭。
2. **测试建模偏差**：`test_early_response_is_relayed` 最初用 `read_to_end` 等待 EOF，与真实客户端行为（读到响应即关闭）不符，导致误判。改为"读到完整响应即断言"，上游改为"读完请求头后响应但不读请求体、保持连接"。
3. **MSRV 修正**：代码使用 let-chains 与 `is_multiple_of`，实际最低版本为 1.88，`rust-version` 与两份 README 已同步更正（原写 1.85）。
4. **保持回滚项不回退**：上一轮移除的 `panic = "abort"` 未重新引入，避免单连接 panic 终止整个进程。

### 9.3 验证结果

| 项目 | 结果 |
|------|------|
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --all-targets -- -D warnings` | 0 warning |
| `cargo test` | **46 passed**（原 30），含 relay 半关闭、提前响应、chunked 透传、走私拒绝、流式 256 KiB body、per-IP 限额、认证缓存/并发、DNS 缓存 |
| 端到端（本地回环） | HTTP GET / 200 KB POST / SOCKS5 / CONNECT 隧道均正常；认证无凭据 `407`、有凭据 `200`，连续请求命中缓存 |
| 配置摘要 | 新字段全部输出，且不含任何凭据 |

### 9.4 与计划的差异

- 计划中的"per-IP 限额默认开启"改为**默认 0 关闭**（避免误伤 NAT），见 R5。
- HTTP 请求体上限与 `413` 按要求移除；`431`（头过大）、`400`（走私）保留。
- 其余项按计划实施，§8 暂缓项不变。

---

## 10. 第二轮：暂缓项全部落地

### 10.1 `httparse` 替换手写解析

- 新增 `src/proxy/http1.rs`：请求头与响应头统一用 `httparse` 解析（零拷贝、支持分段输入）。
- `BufferedConnection` 按"够一段就试解析"的方式喂数据，解析成功后把剩余的 body 字节 `unread` 回缓冲，不丢字节。
- 删除手写行解析（`read_line` / `read_line_with_limit` / `MAX_LINE_LENGTH`）及其测试。
- 行为不变：头数超过 100 → `431`；解析失败 → `400`；超时仍然生效。

### 10.2 上游 keep-alive 连接池

- 新增 `src/proxy/pool.rs`：`ConnectionPool`（按 `host:port` 分组、每 host 上限、空闲 TTL、总 host 上限 1024）。
- 复用前用 `try_read` 探测：有数据或 EOF 都视为不可复用（避免把脏连接交给下一个请求）。
- `src/proxy/http1.rs::relay_response` 负责响应定界：状态行原样转发、逐跳头剥离、注入 `Connection: close`，然后按 `Content-Length` / chunked / 无体（HEAD、204、304、1xx）/ 读到 EOF 转发 body，并返回 `reusable`。
- 只有"请求体已完整发出 + 响应可定界 + 未 `Connection: close`"才回池；`BufReader` 缓冲非空则丢弃连接（保守策略）。
- 陈旧池连接写入失败时**重试一次**新连接（此时请求体尚未消费，重试安全）。
- 客户端侧仍固定 `Connection: close`：不实现客户端 keep-alive，语义与第一轮一致。

### 10.3 异步日志

- 新增 `src/common/log_writer.rs`：实现 `log4rs::append::Append`，把记录转成拥有所有权的结构，经**有界队列**（8192）交给专用线程写盘；队列满时丢弃并计数，绝不阻塞调用方。
- `flush()` 发送 ack 消息并等待（2 s 超时），`main` 在退出前调用 `log::logger().flush()` 保证收尾。
- 配置开关 `log.non_blocking`（默认 `true`）；关闭后回到同步写入。
- `Record::target()` 不是 `'static`，通过一次性字符串驻留（数量等于模块数）解决，内存有界。

### 10.4 指标与健康检查

- 新增 `src/common/metrics.rs`：`AtomicU64` 计数器 + `/metrics`（Prometheus 文本）+ `/healthz` + `/` 索引；单请求短连接、5 s 读超时、8 KiB 请求上限。
- 埋点位置：接入层（连接/拒绝）、认证层（失败次数）、两种代理（请求数、CONNECT 数、上游错误、字节数、连接池命中/未命中）。
- 配置 `metrics_address`（默认空 = 关闭）；绑定失败直接报错退出，避免"以为开了其实没开"。

### 10.5 新增配置与依赖

| 配置项 | 默认 | 说明 |
|--------|------|------|
| `upstream_keepalive` | `true` | 跨客户端连接复用上游 HTTP 连接 |
| `upstream_pool_size` | `8` | 每 host 空闲连接上限（0 = 关闭） |
| `upstream_idle_timeout` | `30` | 空闲连接保留秒数 |
| `metrics_address` | `""` | 指标/健康检查监听地址 |
| `log.non_blocking` | `true` | 异步写日志 |

依赖新增 `httparse`、`anyhow`（log4rs `Append` 的返回类型）。

### 10.6 风险与验证

| 风险 | 处置 |
|------|------|
| 响应定界错误导致串包/漏响应 | 5 种定界各有单测（Content-Length、chunked、HEAD、204/304、读到 EOF、中间 1xx）；不可定界的响应一律不复用 |
| 复用已被对端关闭的连接 | `try_read` 探测 + 写入失败重试一次 |
| 异步日志丢日志 | 有界队列 + 丢弃计数 + 退出前 `flush()`（ack 同步） |
| 日志线程与进程退出竞争 | writer 线程在 channel 关闭后自行 `flush`；`main` 显式 flush |
| 指标端点暴露信息 | 默认关闭；仅暴露计数，不含目标地址与凭据 |

验证结果：

| 项目 | 结果 |
|------|------|
| `cargo fmt --check` / `clippy -D warnings` | 通过 / 0 warning |
| `cargo test` | **64 passed**（新增：http1 解析与 5 种定界、连接池 5 项、指标路由与渲染、异步日志 2 项、连接池复用端到端） |
| 端到端（本地回环） | 3 次独立 curl → 上游只建立 **1** 条连接（`conn=1 req=1..3`），`pool_hits=2 / pool_misses=1`；`/healthz` 返回 `ok`，`/metrics` 输出正常 |
| 回归 | HTTP GET/POST(100 KB)/SOCKS5/CONNECT 全部正常，日志文件按预期落盘且无 ERROR/WARN |

---

## 11. 第三轮：完整复核（全量通读 + 端到端）

本轮不新增功能，只做复核；仍发现并修复了 9 处实现缺陷，并修正 4 处文档不一致。

### 11.1 修复的实现缺陷

| # | 缺陷 | 影响 | 修复 |
|---|------|------|------|
| 1 | `write_head` 用 `\r\n` 定位状态行结尾 | 上游用裸 LF 结束时定位失败，状态行与全部头被重复写出，响应损坏 | 改为定位首个 `\n` |
| 2 | 1xx 中间响应也被注入 `Connection: close` | 语义错误，个别客户端可能据此提前断开 | 仅最终响应注入 |
| 3 | `Transfer-Encoding` 存在但非 chunked 时按 Content-Length 处理 | 定界错误可能导致串包 | 任何 TE 非 chunked → 按连接关闭定界（RFC 9112 §6.3） |
| 4 | chunked 终止符/尾部只接受 `\r\n` | 宽松 LF 上游会导致请求失败 | 同时接受 `\n` |
| 5 | 认证失败指标重复计数（错误口令时 `auth_failure()` 调两次） | 指标虚高 | 统一在函数末尾计数一次，并加回归测试 |
| 6 | 请求体转发失败时未半关闭上游写端 | 上游可能继续等待 body，直到 keepalive 超时才响应 | `pump_body` 失败路径先 `shutdown()` 上游写端 |
| 7 | 池命中/未命中在"取到池连接"时就记账 | 池连接失效并重连时仍记命中，命中率虚高 | 请求头写成功后再记账，重连成功记未命中 |
| 8 | `non_blocking` 只包装文件 appender | stderr 仍同步写，重定向到文件时同样阻塞 runtime | stderr 也走异步写入线程 |
| 9 | `log.file_size` 无上界 | `file_size * 1024 * 1024` 在极端配置下溢出 | 校验 1–1048576 MB |

另清理了 5 个未使用的连接辅助方法（`drain_buffer`、`clear_buffer`、`flush`、`shutdown`、`has_data`），删除了相应的 `#[allow(dead_code)]`。

### 11.2 文档修正

1. Docker 示例基础镜像 `rust:1.85-alpine` → `rust:1.88-alpine`（与 MSRV 一致，否则构建失败）。
2. 错误码表格移除 `413`（请求体已改为流式转发，代理不再产生该状态码）。
3. 项目结构树补全 `http1.rs`、`pool.rs`、`options.rs`、`log_writer.rs`、`metrics.rs`、`refactor.md`。
4. 逐项核对 12 个指标名、22 个配置项与两份 README 的覆盖关系（无遗漏、无拼写差异）。

### 11.3 验证结果

| 项目 | 结果 |
|------|------|
| `cargo fmt --check` | 通过 |
| `cargo clippy --all-targets -D warnings` | 0 warning |
| `cargo test` | **68 passed** |
| `cargo build --release` | 通过 |
| 端到端 | 3 次 GET 复用同一上游连接（`pool_hits` 正确）；HEAD 无 body 且保留 `Content-Length`；chunked 响应正确透传；无长度响应按连接关闭转发；1xx + 200 正常；1 MB POST 正常；`/healthz`、`/`、`/metrics` 200、未知路径 404 |
