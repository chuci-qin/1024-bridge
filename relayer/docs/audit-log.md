# Relayer 审计日志

> 记录历次审计中发现的问题、根因分析、最终采取的方案与涉及代码位置。
> 按严重等级归类，每条以"问题 → 根因 → 解决方案 → 涉及文件"四段式呈现。
>
> 上次更新：2026-04-18（H0 升级为三段式 sig 流水线 + DLQ）

---

## C / Critical

### C1 —— "Nonce stuck death loop"（已修复）

**问题**
EVM 链上一笔 confirm tx 因 mempool 滞留 / RPC 黑洞被判 stale 后，
原代码只会调 `replace_stale_tx`（同 nonce + 12% gas）。如果 replacement
也失败（比如链上 nonce 已经被别的 relayer 处理掉了），事件文件**永远**
不会被删，submitter 每轮重复尝试同一个废 nonce，整个账户的 nonce 序列
被卡死，后续所有 confirm tx 都无法上链。

**根因**
`replace_stale_tx` 失败的两种主要场景：
1. 链上 nonce 已被自己旧 tx 用掉但 RPC 还看不到 receipt（暂态）
2. 链上 nonce 已被别的 relayer 的 tx 占用（永久态）

二者从 RPC 错误码上很难区分；过去都按"再试一次"处理，永远等不到收敛。

**解决方案（最终版）**
1. **Stage A 入口先 `check_nonce_processed`**，如果链上 nonce 已被处理
   就直接 `delete_pending_event`，根本不进 broadcast。
2. Stage B 判 `Lost` 后，先尝试 `replace_stale_tx`；
3. 失败则 fallback `send_self_transfer_to_unblock`：发一笔 `to=self / value=0 / 同 nonce / +12% gas` 自转，强制把这个 nonce 顶过去；
4. 自转广播完成后**主动删事件文件**，下一轮该事件以全新 nonce 重新进入 Stage A。

**涉及文件**
- `src/main.rs` — `handle_stale_pending_tx`、`handle_failed_replacement`
- `src/evm/submitter.rs` — `send_self_transfer_to_unblock`、`replace_stale_tx`、`check_nonce_processed`

---

## H / High

### H0 —— SVM poller 在 `getTransaction` 拉不到 logs 时静默丢账（已修复，v2 升级为三段式流水线）

**问题**
`poll_svm_events` 流程是两段式：先 `getSignaturesForAddress` 拉一批签名，
再对每个签名调 `getTransaction` 解析 logs。原代码对**单个签名拉详情失败**
只打一行 `warn!` 然后 `continue`，但循环外的 `newest_sig` 是基于本批
最新签名一开始就算好的，最终仍然返回 `Some(newest_sig)`，
**调用方据此推进 checkpoint 跨过了失败的那个签名**。
若那笔 tx 恰好包含 `StakeEvent`，则该跨链转账永久丢失。

更隐蔽的两个变体（连 `warn!` 都没有）：
- `Ok(tx)` 但 `tx.meta == None` —— 节点剪枝 transaction history
- `Ok(tx)` 但 `tx.meta.log_messages == None` —— 节点配置裁掉 logs

**根因**
- `getSignaturesForAddress` 返回的所有 sig 都调用过桥合约
- 拿不到完整 logs ≠ "tx 里没事件"，而是 "**我们看不到里面是什么**"
- 但代码假设了"看不到 = 没事件"，从而前进 checkpoint

**解决方案（v1：已废弃）**
把 `poll_svm_events` 里加 `had_fetch_failure` flag，任何一笔拉不到 →
整轮不推进 checkpoint。副作用：一条永久性不可恢复的 sig 会把整个 poller 卡死。

**解决方案（v2：当前）—— 三段式 sig 流水线 + DLQ**

把原来合一的 `run_svm_poller` 拆成三个独立 task：

| Task | 职责 | 主循环 |
|------|------|--------|
| A: sig enumerator | `getSignaturesForAddress` → 空文件写 `sigs/{chain_id}/` → 推进 checkpoint | 每 POLL_INTERVAL |
| B: event extractor | 读 `sigs/` → `getTransaction` → 提取事件 → 写 `events/` → 删 sig | jittered 1-2s |
| C: submitter | 不变 | jittered 1-2s |

关键机制：
1. **checkpoint 语义**从"已处理到此 sig"变为"已枚举到此 sig"。提取/未提取状态由 `sigs/` 工作队列承载。
2. **有界重试 + DLQ**：extractor 对同一 sig 最多 `SVM_EXTRACT_MAX_ATTEMPTS=10` 次（每次间隔 ≥30s），
   超阈值后 `fs::rename` 到 `sigs_dead/{chain_id}/`，`error!` 一行含 sig / attempts / last_error，便于告警。
3. **sig 文件格式**：空文件（0 字节），文件名 = base58 sig，无扩展名。attempt 元数据全部 in-memory。
4. **崩溃一致性**：空文件天然原子（`File::create_new` = O_CREAT|O_EXCL），无 `.tmp` 中间路径。
5. **进程重启**：in-memory attempt 状态丢失，active sig 全部按 fresh 起算（接受的代价，换取零序列化）。

**运维 SOP**
- 查看 DLQ：`ls sigs_dead/<chain_id>/`
- 复活一条：`mv sigs_dead/<chain_id>/<sig> sigs/<chain_id>/`（extractor 自动按 fresh 处理）
- 批量复活：`mv sigs_dead/<chain_id>/* sigs/<chain_id>/`
- Forensic（attempts / 具体错误）：到日志搜 `warn!` / `error!` 行，关键词为 sig 的 base58 值

**不变量**
- DLQ 中的 sig = 必须人工兜底。extractor 绝不会自动跳过或跨过它。
- H0 的"不静默丢账"保证仍然成立：sig 要么在 active 队列被成功提取，要么在 DLQ 等待人工处理。

**涉及文件**
- `src/svm/sig_queue.rs`（新增）— `AttemptState`、`save_new_sig`、`list_active_sigs`、`delete_sig`、`move_to_dead_letter` + 4 单测
- `src/svm/poller.rs` — 拆 `poll_svm_events` 为 `enumerate_new_signatures` + `fetch_and_extract_events`；保留 `classify_tx_logs` / `SigLogsOutcome`
- `src/main.rs` — 新增 `run_svm_sig_enumerator` + `run_svm_event_extractor`；SVM 链 spawn 3 task
- `src/config.rs` — 新增 `sigs_dir()` / `sigs_dead_dir()`

---

### H1 —— Concurrent submission 导致的脏状态（已修复）

**问题**
最初版本 listener 收到事件后直接走 channel 给 submitter 并发处理，
若 submitter 中途崩溃或 channel 丢消息，事件就永久丢失。

**解决方案**
改造为"listener 收到事件 → 写文件 → submitter 扫文件夹 → 处理成功后删
文件"的解耦模型。每条链独立 poller + submitter，**没有任何 in-memory
queue**。这是后续所有架构升级的基础。

**涉及文件**
- `src/pending_events.rs`（新增）
- `src/main.rs` 全 spawn 流程改写

---

### H2 —— 源链 reorg 让 relayer 处理"幽灵事件"（已修复）

**问题**
EVM poller 直接读 `latest` 块的 logs，源链回滚一两块就会造成
relayer 把已被 reorg 的事件发到 target 链，事后无法回滚。

**解决方案**
统一改为 `safe_head = latest - confirmations` 后再 `eth_getLogs`，
其中 `confirmations` 来自链注册表（ETH 12 / Sepolia 6 / Arbitrum 20 /
Base 10），可被 `EVM_CONFIRMATIONS_<chain_id>` 环境变量覆盖。

SVM 端等价做法是统一使用 `CommitmentConfig::finalized()`。

**涉及文件**
- `src/evm/poller.rs`
- `src/svm/poller.rs`
- `src/chain_registry.rs::confirmations`

---

### H3 —— Pipelined submit 之前每事件阻塞 ~12 块（已修复）

**问题**
旧 submitter 每笔 confirm tx 都同步等到 N confirmations 才处理下一笔，
ETH 上单事件至少要 ~144s（12 块 × 12s）；同一目标链多个 relayer 实例
更容易在该窗口堆积 mempool 冲突。

**解决方案**
拆成 Stage A（broadcast）+ Stage B（async maturity check）两段：
- Stage A 广播完立刻写盘并继续下一笔
- Stage B 在后续轮次复用 `PendingEntry.submission` 异步检测 receipt
- finality 检测期间不阻塞其它事件

EVM 单 relayer 端到端等待时间从 ~144s 降到 ~RPC 延迟级别（数百毫秒）。

**涉及文件**
- `src/main.rs::process_evm_entry`、`process_svm_entry`
- `src/pending_events.rs::Submission`

---

## M / Medium

### M1 —— SVM lazy fetch 持续失败只 warn 不告警（已修复）

**问题**
SVM submitter 启动后第一次广播前需要从链上读 `BridgeState`（拿 USDC mint /
token program）。若 program_id 配错或 RPC 长期不通，`fetch_svm_config`
会**每轮**只打 `warn!`，运维不会被告警系统注意到。

**解决方案**
新增计数器 `consecutive_fetch_fails`，每次失败 +1；连续达到
`SVM_LAZY_FETCH_ERROR_EVERY = 30`（约 1 分钟）就升级为 `error!`，
并在恢复时打 `info!` 标记 `recovered_after_fails`。

**涉及文件**
- `src/main.rs::run_svm_submitter`

---

### M2 —— Pre-broadcast simulation 缺失导致 gas 浪费（已修复）

**问题**
多 relayer 场景下，三个 relayer 同时收到事件就会三个都 broadcast。
即便链上合约会 revert 后两笔，broadcast 仍要花 base fee + revert gas。

**解决方案**
在 broadcast 之前用 `eth_estimateGas`（EVM）/ `simulateTransaction`
（SVM）做一次 preflight：
- revert → skip 不广播
- success → 广播

加上 Stage A 入口的 `check_nonce_processed` 预检，多 relayer
gas 浪费降到接近 0。

**涉及文件**
- `src/evm/submitter.rs::simulate_confirm`
- `src/svm/submitter.rs::simulate_confirm_event`

---

### M3 —— 全链共享一个 stale 阈值（已修复）

**问题**
最初 `STALE_PENDING_TX_SECS` 是个全局常量。L1（ETH 12s/block）和 L2
（Arbitrum 250ms/block）按同一阈值处理，L2 经常误触发 stale 浪费 RPC。

**解决方案**
把 `stale_pending_tx_secs` 加入 `ChainInfo`，按链分档：
ETH/Sepolia 600s / Arbitrum 60s / Base 120s。
环境变量 `EVM_STALE_PENDING_TX_SECS_<chain_id>` 可继续按链覆盖。

**涉及文件**
- `src/chain_registry.rs`
- `src/main.rs::STALE_PENDING_TX_SECS_FALLBACK`

---

### M4 —— EVM `check_tx_maturity` 每轮 N 次冗余 receipt 调用（已修复）

**问题**
每事件每轮都调 `eth_getTransactionReceipt` 一次。对于 ETH 主网 confs=12
的场景，单 tx 在 ~96s 等待期内就会触发 ~96 次 RPC，多事件 + 多 relayer
对付费节点 quota 压力很大。

**解决方案（fast-path）**
`check_tx_maturity(latest_block, cached_mined_block: Option<u64>)`：
- 缓存命中 → `depth = latest - mined_block + 1`，直接判定，**不调 RPC**
- 缓存未命中 → fallback slow path 调 receipt

`mined_block` 在第一次 receipt 返回时写入 `Submission.mined_block` 持久
化，重启后也能继续 fast path。安全性兜底是 Confirmed 分支必须再做一次
`check_nonce_processed`，reorg 让缓存失效时会被检测到。

**涉及文件**
- `src/evm/submitter.rs::check_tx_maturity` + 5 条新单测
- `src/main.rs::process_evm_entry` 调用点传入 `sub.mined_block`

---

### M5 —— SVM `RpcClient` 阻塞 tokio 工作线程（已修复）

**问题**
之前用同步版 `solana_client::rpc_client::RpcClient`，每次 RPC 都会
block 整个 tokio worker，慢节点会拖累其它链的 poller / submitter。

**解决方案**
全面切换到 `solana_client::nonblocking::rpc_client::RpcClient`，所有
调用点 `.await`。

**涉及文件**
- `src/svm/poller.rs`
- `src/svm/submitter.rs`
- `src/discovery.rs`
- `src/main.rs::run_svm_*`

---

## L / Low（已修复）

### L1 —— `let log = || ...` 闭包冗余 + warn 缺上下文

`process_*_entry` 等函数内重复定义 `let log = ||`，且很多
`warn!`/`error!` 现场只打了一段消息没带 chain_id / source_chain_id /
nonce，出问题时难以从日志定位是哪条链 / 哪条事件。

**修复**：删除所有 `let log` 闭包，改为函数顶部把 `source_chain_id`、
`nonce` 提取为局部变量；所有 `info!`/`warn!`/`error!`/`debug!`
现场用结构化字段补齐 `chain_id`、`source_chain_id`、`nonce` 三件套。

**涉及文件**：`src/main.rs::process_svm_entry / process_evm_entry /
handle_stale_pending_tx / handle_failed_replacement`

---

### L2 —— `logs_dir` 重复 mkdir

`Config::ensure_dirs` 已经保证 logs 目录存在，`logging::init` 又
调一次 `create_dir_all` 是死代码。

**修复**：从 `logging::init` 移除冗余调用。

**涉及文件**：`src/logging.rs`

---

### L3 —— `error.rs` 模块未被使用

定义了一套自定义 `thiserror` 错误，但全代码库 `Result` 都用 `anyhow`，
该模块从未被引用。

**修复**：删除 `src/error.rs` 与 `Cargo.toml` 中的 `thiserror` 依赖。

**涉及文件**：`src/error.rs`（删除）、`Cargo.toml`、`src/main.rs`（去掉 `mod error`）

---

### L4 —— 1024 testnet RPC URL 误带 `/rpc/` 后缀

`chain_registry.rs` 中 `1024_TESTNET` 的 default_rpc 末尾多了 `/rpc/`，
与其它 1024 RPC 不一致，运维容易误以为需要带后缀。

**修复**：改为 `https://rpc-testnet.1024chain.com`，并加单测
`chain_1024_rpc_urls_have_no_rpc_suffix` 防止回归。

**涉及文件**：`src/chain_registry.rs`

---

### L5 —— Solana 链 chain_id 占位为 0

`solana / solana_devnet` 的 chain_id 在 deploy 脚本里是 0 占位，
跨链时不能与其它链区分。

**修复**：按 SLIP-0044 / 跨链桥惯例分配
`solana=101 / solana_devnet=103`，与 `chain_registry.rs` 对齐，
deploy/common.sh CHAIN_ID 表也同步更新。

**涉及文件**：
- `src/chain_registry.rs`
- `deploy/common.sh`（在第二个 commit 中）

---

## 关闭 / 撤销的提议

| ID  | 提议                                          | 结论                                                                 |
| --- | --------------------------------------------- | -------------------------------------------------------------------- |
| M-X | 加防御性 `assert!` 兜底（早期 M2）            | 撤回——`anyhow::Context` 已经足够，`assert!` 在 release 模式无意义     |
| M-Y | 用 `getProgramAccounts` 优化 peer 发现        | 撤回——主网 RPC 多数已禁用 `getProgramAccounts`，现有按 PDA 拉取够用 |
| M-Z | SVM 缓存 pending slot 减少 RPC                | 撤回——SVM 用 finalized commitment + 短确认时间，RPC 节省微乎其微      |
| —   | EVM 之外做 SVM fast-path                      | 撤回——SVM 不是 N-block 模型，节省的 RPC 只有个位数              |
| —   | pending 事件文件向后兼容（schema 升级路径）   | 撤回（用户决策）——尚未上线，不需要兼容旧格式                           |

---

## 运维侧的小坑（备忘）

1. **首次启动**会在 `${DATA_DIR}/keys/` 自动生成密钥并写 `addresses.json`，
   必须把 `svm_pubkey` + `evm_address` 加入 1024 链桥合约的 relayer 白名单
   后才能正常 confirm，否则所有 confirm tx 会被合约 revert（看日志能看到
   `NotARelayer` / `Unauthorized`）。
2. **多 relayer 部署**不需要任何额外配置，每个进程独立的 `DATA_DIR` 即可，
   gas 浪费由 simulation + check_nonce_processed 控制。
3. **重启**：所有持久化都在 `DATA_DIR`，进程冷启动即恢复 pending tx
   状态（通过 `Submission` 字段）；杀进程不会丢事件。
4. **手动跳过 / 重处理某事件**：直接动 `events/<target>/<src>_<nonce>.json`
   即可（删除 = 跳过，删 `submission` 字段 = 重新走 Stage A 广播）。
5. **跳过整段历史**：手动改 `checkpoints/<chain_id>.json` 的 block / sig
   到目标位置即可，poller / sig enumerator 下一轮从该位置之后开始。
6. **SVM sig 提取失败**：查看 `sigs_dead/<chain_id>/`，按需
   `mv sigs_dead/<chain_id>/<sig> sigs/<chain_id>/` 复活；
   到日志搜 sig 的 base58 值查看失败原因。
