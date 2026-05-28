# Relayer Architecture

> 与上一版（inbound/outbound 双进程 + 同步等待 finality）相比，本版做了三件大事：
>
> 1. 把 1024 链 / EVM peer / SVM peer 都拍平成同一类对象 `ChainEndpoint`，每条链一个 poller + 一个 submitter，**不再有 in/out 之分**。
> 2. 把"广播 → 等 finality → 删事件"拆成两段，**pipelined**：广播完立刻处理下一笔，确认走异步轮询。
> 3. 把所有有状态信息（pending tx、mined block、最后处理签名等）都写进**事件文件本身**，重启即恢复。

---

## 1. 顶层流程

```text
┌─────────────────────────────────────────────────────────────────┐
│  N 条链：1024 + 每个 EVM peer + 每个 SVM peer                    │
│                                                                 │
│   ┌──────────┐ events_dir/<target_chain>/<src>_<nonce>.json     │
│   │ Poller   │ ───────────────────────────────►   ┌──────────┐  │
│   │ (per ch) │                                     │Submitter│  │
│   └──────────┘                                     │(per ch) │  │
│        │                                           └──────────┘  │
│        │ checkpoints/<chain_id>.json                    │        │
│        │                                                ▼        │
│        ▼                                       confirm_event tx  │
│   推进进度                                          (target chain)│
└─────────────────────────────────────────────────────────────────┘
```

- EVM 链独立启动 1 个 poller 协程 + 1 个 submitter 协程（共 2 task）。
- SVM 链独立启动 3 个协程：sig enumerator + event extractor + submitter。
- 慢链 / 慢 finality 不会阻塞其它链，因为协程之间只通过文件系统通信。
- 所有 task **没有任何共享内存**；唯一接口是 `events_dir/`、`sigs/`、`sigs_dead/` 子目录。

---

## 2. 关键抽象

### 2.1 `ChainEndpoint`（`src/chain_endpoint.rs`）

把 EVM 与 SVM 链统一成一个枚举（含 chain_id / kind / rpc / contract 等），main.rs 在
启动时为每个 endpoint spawn 对应的 poller + submitter。

### 2.2 `PendingEntry` 与 `Submission`（`src/pending_events.rs`）

事件文件的 schema：

```rust
struct PendingEntry {
    event: StakeEventData,
    submission: Option<Submission>,   // 已广播但尚未 final
    created_at: SystemTime,
}

enum Submission {
    Evm  { tx_hash, nonce, broadcast_at, mined_block: Option<u64> },
    Svm  { signature, broadcast_at },
}
```

- 文件名：`<source_chain_id>_<nonce>.json`（保证幂等）
- 写入：`tmp + atomic rename`（`pending_events::save_pending_entry`）
- 状态机：`None` → `Some(Submission)` → 文件被删除

### 2.3 链注册表（`src/chain_registry.rs`）

`ChainInfo { chain_id, env_name, default_rpc, kind, confirmations, stale_pending_tx_secs, svm_program_kind }`。

| 链              | chain_id | confirmations | stale_pending_tx_secs | svm_program_kind |
| --------------- | -------- | ------------- | --------------------- | ---------------- |
| Ethereum        | 1        | 12            | 600                   | —                |
| Sepolia         | 11155111 | 6             | 600                   | —                |
| Arbitrum        | 42161    | 20            | 60                    | —                |
| Arbitrum Sepolia| 421614   | 20            | 60                    | —                |
| Base            | 8453     | 10            | 120                   | —                |
| Base Sepolia    | 84532    | 10            | 120                   | —                |
| Solana mainnet  | 101      | 0 *           | 0 *                   | Leaf             |
| Solana devnet   | 103      | 0 *           | 0 *                   | Leaf             |
| 1024 mainnet    | 91024    | 0 *           | 0 *                   | Hub              |
| 1024 testnet    | 91025    | 0 *           | 0 *                   | Hub              |
| 1024 stablenet  | 91026    | 0 *           | 0 *                   | Hub              |

\* SVM 链不走"等 N 块"模型，统一使用 `CommitmentConfig::finalized()` 与
   `STALE_PENDING_SVM_TX_SECS` 常量。

按链可被环境变量覆盖：

- `RPC_<env_name>` —— RPC URL
- `EVM_CONFIRMATIONS_<chain_id>` —— 紧急加保守
- `EVM_STALE_PENDING_TX_SECS_<chain_id>` —— 调 stale 阈值

`svm_program_kind` 在 SVM 链上区分两套 Anchor 程序，**完全由 chain_registry 内置**，
没有环境变量入口（程序版本是 ABI 的一部分，不能跨链漂移）：

- `Hub`（部署在 1024 链）—— `contracts/svm/programs/bridge1024_hub`，多 peer，
  含独立的 `PeerConfig` PDA；`BridgeState` 只存 hub 自身配置。
- `Leaf`（部署在 Solana 等叶子链）—— `contracts/svm/programs/bridge1024`，
  单 peer，peer 元数据内嵌在 `BridgeState` 里，无 `PeerConfig` PDA。

详见 §2.4。

### 2.4 SVM Hub / Leaf 分发（`src/types.rs::SvmProgramKind`）

EVM 那一侧只有一个 `Bridge1024.sol` 合约，所有 EVM peer 复用同一份代码。
SVM 那一侧因为 hub 需要管理多 peer、leaf 只服务一条对端，被拆成了两套 Anchor
程序，**relayer 用 `SvmProgramKind { Hub, Leaf }` 枚举来分发逻辑**：

| 方面              | Hub（1024 链）                                              | Leaf（Solana 等叶子链）                       |
| ----------------- | ----------------------------------------------------------- | --------------------------------------------- |
| `BridgeState` 字段 | 只包含 hub 自身配置（admin / nonce / token / paused / …）   | 额外内嵌单 peer 配置（peer_chain_id / address / …） |
| `PeerConfig` PDA  | 有，每个对端一个                                            | 无                                            |
| `CrossChainRequest` PDA seeds | `[b"cross_chain_request", source_chain_id LE, nonce LE]` | `[b"cross_chain_request", nonce LE]`          |
| `confirm_event` 账户数 | 10（含 `peer_config`）                                  | 9（无 `peer_config`）                         |
| `confirm_event` 指令数据 | `[8B disc][Borsh(BridgeEventData)]`（184B）              | 与 Hub 完全一致（184B）                       |

`SvmProgramKind` 在启动时由 `chain_registry::svm_program_kind(chain_id)` 决定，
通过 `SvmConfig.program_kind` 字段一路下发到：

- `discovery::fetch_bridge_state(..., kind)` —— 用 `HubBridgeStateData` /
  `LeafBridgeStateData` 选对应的 Borsh 形状反序列化；
- `svm::submitter::cross_chain_request_pda(..., kind)` —— 选 2 或 3 个 seed；
- `svm::submitter::build_confirm_event_instruction(..., kind)` —— 选 9 或 10
  account 布局；
- `svm::submitter::broadcast_confirm_event(..., kind)` / `check_nonce_status(..., kind)`
  —— 统一携带 kind，避免 nonce 检查与提交用不同 PDA 形状导致幽灵 pending。

所有差异都集中在以上 4 个函数里，新加链只要在 `chain_registry` 标好 `Hub` 或
`Leaf` 即可，无需改动 submitter / discovery。

---

## 3. Poller

### 3.1 EVM (`src/evm/poller.rs`)

```text
loop:
    latest = eth_blockNumber
    safe   = latest - confirmations            ← reorg 边界，只读 safe 之前的
    from   = checkpoint + 1
    to     = min(safe, from + EVM_LOG_PAGE_SIZE - 1)
    if to < from: sleep(EVM_POLL_INTERVAL); continue
    logs   = eth_getLogs(from..=to, StakeEvent)
    for log in logs:
        write events_dir/<target>/<src>_<nonce>.json
    write checkpoint = to
    sleep(EVM_POLL_INTERVAL)
```

- **不读 latest** 直接读 `safe = latest - confirmations`，源链 reorg 不会让 relayer
  处理"假事件"。
- 单页拉满 `EVM_LOG_PAGE_SIZE` 条，避免 RPC 一次返回过大被节点限流。

### 3.2 SVM（三段式 sig 流水线 + DLQ）

SVM 链的 poller 拆成两个独立 task，用磁盘 sig 队列解耦：

```text
┌────────────────────────────────┐      ┌────────────────────────────────┐
│  Task A: sig enumerator        │      │  Task B: event extractor       │
│                                │      │                                │
│  loop:                         │      │  loop:                         │
│    sigs = getSignaturesFor-    │      │    sleep(jittered 1-2s)        │
│           Address(until=cp)    │      │    for sig in sigs/{chain}/    │
│    for sig in sigs:            │      │      if throttled: skip        │
│      create sigs/{chain}/{sig} │─────►│      tx = getTransaction(sig) │
│    checkpoint = newest sig     │      │      if OK:                    │
│    sleep(POLL_INTERVAL)        │      │        write events/           │
│                                │      │        delete sigs/{chain}/sig │
│                                │      │      if Err × N:              │
│                                │      │        mv → sigs_dead/ + error!│
└────────────────────────────────┘      └────────────────────────────────┘
```

- **checkpoint 语义**变为"已枚举到此 sig"，提取状态由 `sigs/` 队列承载。
- **sig 文件**：空文件，文件名 = base58 signature，无扩展名。attempt 元数据全部 in-memory。
- **DLQ**：`attempt_count >= SVM_EXTRACT_MAX_ATTEMPTS (10)` → `fs::rename` 到 `sigs_dead/{chain_id}/`，`error!` 一行触发告警。
- **手工复活**：运维 `mv sigs_dead/<chain>/<sig> sigs/<chain>/`，extractor 下一轮自动按 fresh 处理。
- **重启**：in-memory attempt 状态丢失，所有残留 active sig 重新从 attempt_count=0 起算。
- 只用 `finalized` commitment，避免回滚误判。

---

## 4. Submitter（pipelined）

每条链一个 submitter 协程，主循环 jittered 1–2s（`jittered_submit_interval`）：

```text
loop:
    list   = events_dir/<my_chain_id>/*.json     ← 我作为 target 的 pending
    shuffle(list)                                 ← 多 relayer 实例分散竞争
    latest = rpc.latest_block (per-round cached)  ← 同一轮所有事件复用，省 RPC

    for entry in list:
        if entry.submission.is_none():
            ── Stage A: 没广播过 ──
            if check_nonce_processed(@safe_head): delete file; continue
            simulate (eth_estimateGas / preflight)
            if revert: warn + skip                 ← 链上别人已成功，下一轮再 check
            tx_hash = broadcast()
            entry.submission = Some(broadcast info)
            save_pending_entry()                   ← 写盘后立即处理下一笔
        else:
            ── Stage B: 已广播过，查成熟度 ──
            match check_tx_maturity(latest, cached_mined_block):
                Confirmed → verify check_nonce_processed → delete file
                Pending   → 跳过等下一轮
                Lost      → goto Stage C (stale 流程)

    sleep(jittered_submit_interval())
```

### 4.1 EVM 成熟度判定（`src/evm/submitter.rs::check_tx_maturity`）

```text
if cached_mined_block.is_some():
    ── Fast path (M-O 优化) ──
    depth = latest - mined_block + 1
    if depth >= confirmations: Confirmed
    else: Pending(mined_block, depth)
else:
    ── Slow path ──
    receipt = eth_getTransactionReceipt(hash)
    if receipt.is_none() && >stale_pending_tx_secs since broadcast: Lost
    else: 用 receipt.block_number 算 depth
```

Fast path 单笔 tx 在 confs=12 期间节省 ~96 次 `eth_getTransactionReceipt`
（每秒约 1 次轮询 × 96s 等待）。安全性兜底是后续的 `check_nonce_processed`
二次校验。

### 4.2 EVM stale 处理（"nonce stuck death loop" 修复）

```text
on Lost:
    if check_nonce_processed(safe_head):
        ── 链上 nonce 已用，但不是我们这笔（被别的 relayer 顶了） ──
        delete file
    else:
        try replace_stale_tx (same nonce, +12% gas):
            on success: 更新 submission, 等下一轮
            on failure (nonce too low / underpriced):
                send_self_transfer_to_unblock(nonce, +12% gas):
                    向自己转 0 ether，把这个账户 nonce 顶上去
                delete file                          ← 释放后下一轮重新 broadcast
```

详细案例与替代方案见 [`audit-log.md`](./audit-log.md) "H1（关键）"。

### 4.3 SVM 成熟度判定（`src/svm/submitter.rs`）

```text
sig_status = get_signature_statuses([sig], commitment=finalized)
match status:
    None / Failed   → 若 >stale_pending_svm_tx_secs: Lost
    Confirmation lvl < Finalized: Pending
    Finalized:
        if status.err.is_some(): 删文件 + warn (链上 revert)
        else: verify nonce_processed(commitment=finalized) → 删文件
```

`fetch_svm_config` lazy 拉取链上 BridgeState；连续 30 轮失败会从
`warn!` 升级到 `error!`，便于触发告警（M1 修复）。

---

## 5. Reorg 抗性

| 风险点                                                       | 防御                                                           |
| ------------------------------------------------------------ | -------------------------------------------------------------- |
| 源链 reorg → relayer 把"幽灵事件"当成有效                    | poller 只读 `safe = latest - confirmations`                    |
| target 链 reorg 删掉了已 mined 的 confirm tx                 | submitter 等 N confirmations 后才视为 Confirmed                |
| target 链 reorg 让 `check_nonce_processed` 返回过早的"已处理" | `check_nonce_processed` 在 `safe_head` 上 eth_call，不在 latest |
| SVM 短 fork                                                  | 全程使用 `CommitmentConfig::finalized()`                       |
| EVM fast-path 缓存的 `mined_block` 因 reorg 失效             | Confirmed 分支强制再做一次 `check_nonce_processed` 二次校验    |

---

## 6. 多 relayer 协同（gas / 资源竞争）

- **submitter 周期 jitter 1–2s**（`jittered_submit_interval`）—— 错开提交点
- **pending 文件 shuffle** —— 多个 relayer 不会按同一字典序抢同一笔
- **pre-broadcast simulation** —— 别人已成功的 tx 在 `eth_estimateGas` 阶段
  revert，立即 skip，不上链不烧 gas
- **check_nonce_processed pre-check** —— Stage A 入口先查，避免明知会失败还广播

最坏情况下三个 relayer 同时广播：第一笔被打包，后两笔在
`getTransactionReceipt` 之前就会因 simulation revert 被本进程内挡掉，
即便都上链也只是少量 gas（被合约 nonce check revert）。

---

## 7. 数据目录布局

```
$DATA_DIR/                 # 默认 /data，Docker volume
├── keys/
│   ├── svm_keypair.json     0600，首次启动自动生成
│   ├── evm_wallet.key       0600，首次启动自动生成
│   └── addresses.json       明文，含 svm_pubkey + evm_address，运维抄到合约白名单
├── checkpoints/
│   └── <chain_id>.json      poller / sig enumerator 进度
├── events/
│   └── <target_chain_id>/   pending 事件文件
│       └── <source_chain_id>_<nonce>.json
├── sigs/                    SVM sig 工作队列（空文件，文件名=base58 sig）
│   └── <chain_id>/
│       └── <base58_signature>
├── sigs_dead/               SVM sig Dead Letter Queue（结构同 sigs/）
│   └── <chain_id>/
│       └── <base58_signature>
└── logs/
    └── relayer.log.YYYY-MM-DD   每日轮转，保留 14 天（L4 审计需求）
```

---

## 8. 关键运行时常量

| 常量                            | 值          | 含义                                            |
| ------------------------------- | ----------- | ----------------------------------------------- |
| `EVM_POLL_INTERVAL`             | 5s          | EVM poller 轮询周期                             |
| `EVM_LOG_PAGE_SIZE`             | 2000        | 单次 `eth_getLogs` 最多拉多少块                 |
| `SVM_POLL_INTERVAL`             | 5s          | SVM sig enumerator 轮询周期                     |
| `SVM_EXTRACT_MAX_ATTEMPTS`      | 10          | sig 提取最大尝试次数，超后进 DLQ                |
| `SVM_EXTRACT_MIN_RETRY_INTERVAL`| 30s         | 同一 sig 两次提取 attempt 的最小间隔            |
| `SVM_MAX_SIGS`                  | 1000        | 单次 `getSignaturesForAddress` limit            |
| `SUBMIT_INTERVAL_MIN/MAX`       | 1s / 2s     | submitter jitter 区间                           |
| `STALE_PENDING_SVM_TX_SECS`     | 90s         | SVM 单 tx 等 finality 的最长容忍                |
| `EVM_REPLACEMENT_GAS_BUMP_PCT`  | 12          | replacement / self-transfer gas 加价百分比      |
| `SVM_LAZY_FETCH_ERROR_EVERY`    | 30          | SVM 配置拉取失败多少轮升级成 `error!`           |

EVM 每链的 `confirmations` 与 `stale_pending_tx_secs` 见 §2.3。

---

## 9. 启动顺序（main.rs）

```text
1. Config::from_env()                  ← BRIDGE_1024_PROGRAM_ID + NETWORK + DATA_DIR
2. Config::ensure_dirs()               ← 创建 keys/checkpoints/events/logs
3. logging::init()                     ← stderr + 每日文件双输出，RUST_LOG 控制
4. Keys::load_or_generate()            ← 自动生成 0600，写 addresses.json + WARN
5. discovery::list_peers()             ← 扫 1024 链 PeerConfig PDA，收集所有 peer
6. for endpoint in [self_1024, ...peers]:
       if EVM:
           spawn evm_poller(endpoint)
           spawn evm_submitter(endpoint)
       if SVM:
           spawn svm_sig_enumerator(endpoint)
           spawn svm_event_extractor(endpoint)
           spawn svm_submitter(endpoint)
7. tokio::signal::ctrl_c().await       ← graceful shutdown，所有 task 收到信号后
                                         完成当前一轮再退出
```
