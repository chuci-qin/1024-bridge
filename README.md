# Bridge1024

多签跨链桥系统，支持 EVM 链（Ethereum / Arbitrum / Base 等）与 SVM 链（1024chain / Solana）之间的 USDC 双向跨链转移。

## 架构概览

```
┌────────────────┐    Staked event    ┌──────────────────────┐  confirm_event tx  ┌────────────────┐
│ EVM Contract   │ ─────────────────► │                      │ ─────────────────► │ SVM Program    │
│ Bridge1024.sol │                    │    Relayer (Rust)    │                    │ bridge1024     │
│                │ ◄───────────────── │                      │ ◄───────────────── │ (Anchor)       │
└────────────────┘  confirm_event tx  │  per-chain poller    │    Staked event    └────────────────┘
                                      │  per-chain submitter │
                                      └──────────────────────┘
```

**核心机制**：质押-解锁（Stake-Unlock），非铸币-销毁。用户在源链 Stake USDC → Relayer 多签确认 → 目标链自动 Unlock 等额 USDC。

**多签验证**：2/3 以上白名单 Relayer 签名通过后自动解锁（最多 18 个 Relayer）。

**密码学**：各链使用原生算法 — SVM 端 Ed25519 + Borsh，EVM 端 ECDSA (secp256k1) + EIP-191。Relayer 负责两种格式间的转换。

## 支持的网络

| 环境 | 1024chain | EVM 链 | Solana |
|------|-----------|--------|--------|
| mainnet | 91024 | Ethereum | - |
| stablenet | 91026 | ETH Sepolia, Base Sepolia | Solana (devnet) |
| testnet | 91025 | ETH Sepolia | Solana (devnet) |

部署地址见 `deploy/config/<env>/addresses.json`，Relayer 公钥见 `deploy/config/<env>/relayers.json`。

## 项目结构

```
1024-bridge/
├── contracts/
│   ├── evm/                        # EVM 智能合约 (Solidity, Foundry)
│   │   ├── src/Bridge1024.sol      #   核心合约：stake / unlock / 角色 / 速率限制
│   │   └── test/Bridge1024.t.sol   #   Foundry 测试
│   └── svm/                        # SVM 智能合约 (Anchor, Rust)
│       └── programs/bridge1024/    #   核心程序：多 Peer、PDA 金库、时间锁
├── relayer/                        # 统一 Relayer 服务 (Rust)
│   ├── src/
│   │   ├── main.rs                 #   入口：从链上发现配置，spawn per-chain tasks
│   │   ├── evm/{poller,submitter}  #   EVM 链：轮询事件 + 提交确认
│   │   ├── svm/{poller,submitter,sig_queue}  # SVM 链：签名枚举 + 事件提取 + 提交
│   │   ├── pending_events.rs       #   文件系统事件队列（原子写入 + 状态机）
│   │   ├── checkpoint.rs           #   进度持久化
│   │   ├── discovery.rs            #   从 1024 链上自动发现所有 Peer 配置
│   │   └── chain_registry.rs       #   链参数注册表（confirmations、stale_tx 等）
│   ├── Dockerfile                  #   Docker 镜像
│   └── docs/                       #   Relayer 架构文档、审计日志格式
├── deploy/
│   ├── evm/                        # EVM 部署 / 配置 / 运维脚本 (bash + cast)
│   ├── svm/                        # SVM 部署 / 配置 / 运维脚本 (TypeScript)
│   ├── config/{mainnet,stablenet,testnet}/  # 各环境合约地址 + Relayer 列表
│   ├── bridge.sh                   # 一键 deploy + init 入口
│   └── common.sh                   # 公共工具函数
├── docs/                           # 设计文档、API 文档、测试计划
└── .github/workflows/ci.yml       # CI：EVM (Foundry) + SVM (Anchor) + Relayer (Cargo)
```

## 智能合约

### EVM — `Bridge1024.sol`

Solidity 0.8.20，基于 OpenZeppelin（Pausable、ReentrancyGuard、SafeERC20）。

**核心功能**：
- `stake(amount, receiverAddress)` — 用户锁定 USDC，扣除手续费，发出 `Staked` 事件
- `confirmEvent(eventData, signatures)` — Relayer 提交确认，达到 2/3 阈值自动 `unlock`

**安全机制**：
- 四角色体系：admin（多签）/ guardian（紧急冻结）/ operator（运维）/ recovery（冷钱包恢复）
- 滑动窗口速率限制（单笔上限 + 时间窗口总额）
- 金库最低储备金检查
- Nonce 递增防重放
- 合约即金库（无需外部 vault 或 approve）

### SVM — `bridge1024` (Anchor)

**核心功能**：
- `stake` — 用户质押 USDC，扣除 bridge_fee（per-peer 配置），发出 `Staked` 事件
- `confirm_event` — Relayer 提交 Ed25519 签名确认，达到阈值自动 unlock
- 多 Peer 支持 — 每条对端链独立的 `PeerConfig` PDA

**安全机制**：
- 四角色分离（admin / guardian / operator / recovery）
- 时间锁（24h 延迟 + 48h 执行窗口）
- 双层滑动窗口速率限制（per-chain + 全局）
- PDA 金库（program-owned token account）
- Token-2022 兼容（`token_interface`）

## Relayer

统一 Rust Relayer（v0.3.0），单一二进制，自动发现并服务所有已注册的桥。

**架构特点**：

- 启动时从 1024 链上读取 `BridgeState` 和所有 `PeerConfig`，构造 `ChainEndpoint` 列表
- 每条链独立 spawn 异步 task：
  - **EVM 链**：poller（轮询事件） + submitter（提交确认），共 2 task
  - **SVM 链**：sig enumerator + event extractor + submitter，共 3 task
- Task 之间**零共享内存**，通过文件系统解耦（`events_dir/`、`sigs/`、`sigs_dead/`）
- Pipelined 处理：广播后立刻处理下一笔，确认走异步轮询
- 重启自动恢复：所有状态持久化在事件文件和 checkpoint 中

**链支持**（内置注册表）：

| 链 | Chain ID | Confirmations | Stale TX Timeout |
|----|----------|--------------|-----------------|
| Ethereum | 1 | 12 | 600s |
| Arbitrum | 42161 | 20 | 60s |
| Base | 8453 | 10 | 120s |
| Solana | 101 | - | - |
| 1024 mainnet | 91024 | - | - |
| 1024 stablenet | 91026 | - | - |

（各链的 Sepolia/testnet/devnet 变体同样支持）

## 快速开始

### 前置条件

**Rust**（>= 1.85）：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**EVM 工具链**（Foundry）：

```bash
curl -L https://foundry.paradigm.xyz | bash && foundryup
```

**SVM 工具链**（Anchor + Solana CLI）：

```bash
cargo install --git https://github.com/coral-xyz/anchor avm --locked --force
sh -c "$(curl -sSfL https://release.solana.com/stable/install)"
```

### 编译合约

```bash
# EVM
cd contracts/evm && forge build

# SVM
cd contracts/svm && anchor build
```

### 运行测试

```bash
# EVM 单元测试
cd contracts/evm && forge test

# SVM 测试
cd contracts/svm && anchor test

# Relayer 编译检查
cd relayer && cargo build
```

### 部署

使用 `deploy/` 目录下的脚本，需先配置 `.env` 文件（参考 `deploy/config/<env>/.env.example`）。

```bash
# 一键部署（EVM + SVM + 初始化 + 注册 Relayer）
./deploy/bridge.sh <env>

# 或分步执行
./deploy/evm/deploy.sh         # 部署 EVM 合约
./deploy/evm/configure.sh      # 配置 USDC、对端、角色
./deploy/evm/add-relayer.sh    # 注册 Relayer
./deploy/evm/fund-vault.sh     # 注入流动性

./deploy/svm/deploy.sh         # 部署 SVM 程序
./deploy/svm/initialize.sh     # 初始化
./deploy/svm/configure.sh      # 配置 USDC
./deploy/svm/register-peer.sh  # 注册对端链
./deploy/svm/add-relayer.sh    # 注册 Relayer
./deploy/svm/fund-vault.sh     # 注入流动性
```

### 运行 Relayer

```bash
cd relayer && cargo run --release
```

或使用 Docker：

```bash
docker build -t bridge1024-relayer ./relayer
docker run -d \
  --env-file relayer/.env \
  bridge1024-relayer
```

Relayer 启动后自动从链上发现配置，无需手动指定桥接对。

## 运维脚本

| 脚本 | 用途 |
|------|------|
| `deploy/evm/info.sh` | 查看 EVM 合约状态 |
| `deploy/svm/info.sh` | 查看 SVM 程序状态 |
| `deploy/evm/stake.sh` | 手动发起 EVM → SVM 跨链 |
| `deploy/svm/stake.sh` | 手动发起 SVM → EVM 跨链 |
| `deploy/evm/withdraw.sh` | EVM 侧提现 |
| `deploy/svm/withdraw.sh` | SVM 侧提现 |
| `deploy/evm/manage-roles.sh` | 管理 EVM 合约角色 |
| `deploy/svm/manage-roles.sh` | 管理 SVM 程序角色 |
| `deploy/evm/configure-rate-limits.sh` | 配置 EVM 速率限制 |
| `deploy/svm/configure-rate-limits.sh` | 配置 SVM 速率限制 |
| `deploy/evm/activate-timelock.sh` | 激活 EVM 时间锁 |
| `deploy/svm/activate-timelock.sh` | 激活 SVM 时间锁 |
| `deploy/evm/fund-relayers.sh` | 给 Relayer 充 gas |
| `deploy/svm/fund-relayers.sh` | 给 Relayer 充 SOL |

## 文档

- [docs/design.md](docs/design.md) — 系统设计文档
- [docs/api.md](docs/api.md) — API 接口与模块调用规约
- [docs/testplan.md](docs/testplan.md) — 测试计划
- [docs/evm-changelog.md](docs/evm-changelog.md) — EVM 合约变更日志
- [relayer/docs/architecture.md](relayer/docs/architecture.md) — Relayer 架构详解
- [relayer/docs/audit-log.md](relayer/docs/audit-log.md) — Relayer 审计日志格式
- [contracts/evm/docs/](contracts/evm/docs/) — EVM 合约运维指南、速率限制指南、安全审计
- [contracts/svm/docs/](contracts/svm/docs/) — SVM 合约安全审计

## CI

GitHub Actions 工作流 `.github/workflows/ci.yml` 在 push / PR 到 `main` 或 `xbx` 时自动运行：

1. **Detect Changes** — 按目录变更跳过无关 job
2. **EVM Tests** — Foundry `forge test`
3. **SVM Tests** — Anchor `anchor test`
4. **Relayer Build** — `cargo build` + `cargo clippy`

## 版本历史

| Tag | 说明 |
|-----|------|
| `v0.2.1` | 合并前的旧 main 快照（含独立 deploy workflows 和 s2e listener 拆分） |
