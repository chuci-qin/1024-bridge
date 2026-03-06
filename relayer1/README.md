# Relayer 中继服务

## 概述

Relayer 是 Bridge1024 跨链桥的中继组件，负责监听链上事件、签名并提交证明到目标链。

**技术栈**: Rust + Tokio (异步运行时) + Axum (HTTP服务器)

### 实现状态

**✅ M4 阶段完成（2025-11-16）**
- ✅ **S2E Relayer** (SVM→EVM)：完整实现，端到端测试通过
- ✅ **E2S Relayer** (EVM→SVM)：分离式架构，正在运行

## 架构设计

```
┌────────────────────────────────────────────────────────────────┐
│                      Relayer 系统架构                           │
├────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌───────────────────┐         ┌─────────────────────────────┐│
│  │  s2e-relayer      │         │  e2s (分离式架构)           ││
│  │  (单一进程)       │         │                             ││
│  ├───────────────────┤         │  ┌────────────────────────┐ ││
│  │ 监听: 1024chain   │         │  │ e2s-listener           │ ││
│  │ 方式: HTTP RPC轮询│         │  │ 监听 Arbitrum 事件     │ ││
│  │ 签名: ECDSA       │         │  │ 输出到文件队列         │ ││
│  │ 提交: Arbitrum    │         │  └────────────────────────┘ ││
│  │ 端口: 8083        │         │            ↓                 ││
│  └───────────────────┘         │  ┌────────────────────────┐ ││
│                                 │  │ e2s-submitter          │ ││
│                                 │  │ 读取文件队列           │ ││
│                                 │  │ 签名: Ed25519          │ ││
│                                 │  │ 提交: 1024chain        │ ││
│                                 │  │ 端口: 8082             │ ││
│                                 │  └────────────────────────┘ ││
│                                 └─────────────────────────────┘│
│                                                                 │
│  HTTP API: /health, /status, /metrics                          │
└────────────────────────────────────────────────────────────────┘
```

## 目录结构

```
relayer1/
├── README.md                    # 本文件
├── docker-compose.yml           # Docker 编排配置
├── Dockerfile.s2e               # S2E 镜像构建
├── Dockerfile.e2s-listener      # E2S Listener 镜像构建
├── Dockerfile.e2s-submitter     # E2S Submitter 镜像构建
│
├── s2e/                         # SVM → EVM 中继服务
│   ├── src/
│   │   ├── main.rs              # 服务入口
│   │   ├── listener.rs          # SVM 事件监听 (HTTP RPC轮询)
│   │   ├── signer.rs            # ECDSA 签名器
│   │   ├── submitter.rs         # EVM 交易提交
│   │   ├── api.rs               # HTTP API 服务
│   │   └── config.rs            # 配置管理
│   ├── Cargo.toml
│   └── config.example.env       # 配置模板
│
├── e2s-listener/                # EVM → SVM 监听器
│   ├── src/
│   │   ├── main.rs              # 服务入口
│   │   ├── listener.rs          # EVM 事件监听
│   │   └── config.rs            # 配置管理
│   └── Cargo.toml
│
├── e2s-submitter/               # EVM → SVM 提交器
│   ├── src/
│   │   ├── main.rs              # 服务入口
│   │   ├── signer.rs            # Ed25519 签名器
│   │   ├── submitter.rs         # SVM 交易提交
│   │   ├── api.rs               # HTTP API 服务
│   │   └── config.rs            # 配置管理
│   └── Cargo.toml
│
└── shared/                      # 共享库
    ├── src/
    │   ├── lib.rs
    │   ├── config.rs            # 统一配置结构
    │   ├── types.rs             # 类型定义
    │   ├── error.rs             # 错误处理
    │   ├── logger.rs            # 日志系统
    │   ├── metrics.rs           # Prometheus 指标
    │   ├── gas.rs               # Gas 管理
    │   └── retry.rs             # 重试逻辑
    └── Cargo.toml
```

## 核心特性

### 1. 双服务架构

**为什么需要分离？**

- **依赖冲突**: `ethers` (EVM) 和 `solana-sdk` (SVM) 有不可调和的依赖冲突
- **进程隔离**: 独立进程确保一个方向的故障不会影响另一个
- **密钥安全**: 每个服务使用不同的密钥，降低风险

### 2. 密码学实现

#### S2E (SVM→EVM) 签名流程

```rust
// 1. 监听 SVM 事件（HTTP RPC 轮询）
let event = listener::fetch_stake_event(&config).await?;

// 2. 序列化为 JSON
let json_data = serde_json::json!({
    "sourceContract": event.source_contract,
    "targetContract": event.target_contract,
    "chainId": event.source_chain_id,
    "blockHeight": event.block_height,
    "amount": event.amount,
    "receiverAddress": event.receiver_address,
    "nonce": event.nonce
});
let json_string = serde_json::to_string(&json_data)?;

// 3. 计算 SHA-256 哈希
let hash = Sha256::digest(json_string.as_bytes());

// 4. 应用 EIP-191 前缀并签名
let eth_message = format!("\x19Ethereum Signed Message:\n32{}", 
    String::from_utf8_lossy(&hash));
let signature = sign_ecdsa(&eth_message, &private_key)?;

// 5. 提交到 EVM 合约
submit_to_evm(event, signature).await?;
```

#### E2S (EVM→SVM) 签名流程

```rust
// 1. 监听 EVM 事件（ethers event filter）
let event = listener::fetch_evm_event(&config).await?;

// 2. 序列化为 Borsh 格式
let event_data = StakeEventData {
    source_contract: Pubkey::from_str(&event.source_contract)?,
    target_contract: Pubkey::from_str(&event.target_contract)?,
    source_chain_id: event.source_chain_id,
    target_chain_id: event.target_chain_id,
    block_height: event.block_height,
    amount: event.amount,
    receiver_address: event.receiver_address,
    nonce: event.nonce,
};
let message = borsh::to_vec(&event_data)?;

// 3. Ed25519 签名
let signature = keypair.sign_message(&message);

// 4. 创建 Ed25519Program 验证指令
let ed25519_ix = Ed25519Program::create_instruction(
    &keypair.pubkey(),
    &message,
    &signature.as_ref(),
);

// 5. 提交到 SVM（包含验证指令）
let tx = program
    .request()
    .instruction(ed25519_ix)
    .instruction(submit_ix)
    .send()?;
```

### 3. 事件队列（E2S）

E2S 使用简单的文件系统队列：

```
.relayer/queue/
├── event_1.json
├── event_2.json
└── event_3.json
```

- **listener**: 将 EVM 事件写入队列
- **submitter**: 读取并处理队列中的事件
- **格式**: JSON 文件，按 nonce 命名

## 快速开始

### 前置要求

- Rust 1.70+
- Solana CLI（用于生成钱包）
- 测试代币（SVM: 5+ SOL, EVM: 0.1+ ETH）

### 1. 编译

```bash
# 编译 s2e 服务
cd relayer1/s2e
cargo build --release

# 编译 e2s-listener
cd ../e2s-listener
cargo build --release

# 编译 e2s-submitter
cd ../e2s-submitter
cargo build --release
```

### 2. 配置 S2E

```bash
cd s2e
cp config.example.env .env
```

编辑 `.env` 文件：

```bash
# 服务配置
SERVICE__NAME=s2e
SERVICE__VERSION=0.1.0

# 源链（SVM - 1024chain）
SOURCE_CHAIN__NAME=1024chain
SOURCE_CHAIN__CHAIN_ID=91024
SOURCE_CHAIN__RPC_URL=https://rpc-testnet.1024chain.com/rpc/
SOURCE_CHAIN__CONTRACT_ADDRESS=<SVM程序ID>

# 目标链（EVM - Arbitrum Sepolia）
TARGET_CHAIN__NAME="Arbitrum Sepolia"
TARGET_CHAIN__CHAIN_ID=421614
TARGET_CHAIN__RPC_URL=https://sepolia-rollup.arbitrum.io/rpc
TARGET_CHAIN__CONTRACT_ADDRESS=<EVM合约地址>

# Relayer 密钥
RELAYER__ECDSA_PRIVATE_KEY=<ECDSA私钥，用于EVM签名>

# API 配置
API__PORT=8083

# 日志配置
LOGGING__LEVEL=info
LOGGING__FORMAT=text
```

### 3. 配置 E2S

**e2s-listener:**

```bash
cd e2s-listener
cat > .env <<EOF
SOURCE_CHAIN__NAME="Arbitrum Sepolia"
SOURCE_CHAIN__CHAIN_ID=421614
SOURCE_CHAIN__RPC_URL=https://sepolia-rollup.arbitrum.io/rpc
SOURCE_CHAIN__CONTRACT_ADDRESS=<EVM合约地址>

QUEUE__PATH=.relayer/queue
LOGGING__LEVEL=info
LOGGING__FORMAT=text
EOF
```

**e2s-submitter:**

```bash
cd e2s-submitter
cat > .env <<EOF
TARGET_CHAIN__NAME=1024chain
TARGET_CHAIN__CHAIN_ID=91024
TARGET_CHAIN__RPC_URL=https://rpc-testnet.1024chain.com/rpc/
TARGET_CHAIN__CONTRACT_ADDRESS=<SVM程序ID>

RELAYER__ED25519_PRIVATE_KEY=<Ed25519私钥>

QUEUE__PATH=.relayer/queue
API__PORT=8082
LOGGING__LEVEL=info
LOGGING__FORMAT=text
EOF
```

### 4. 运行

```bash
# 启动 s2e（SVM→EVM）
cd s2e
cargo run --release

# 启动 e2s-listener（EVM→SVM 监听器，新终端）
cd e2s-listener
cargo run --release

# 启动 e2s-submitter（EVM→SVM 提交器，新终端）
cd e2s-submitter
cargo run --release
```

### 5. 验证

```bash
# 健康检查
curl http://localhost:8083/health  # s2e
curl http://localhost:8082/health  # e2s-submitter

# 查看状态
curl http://localhost:8083/status | jq
curl http://localhost:8082/status | jq

# 查看 Prometheus 指标
curl http://localhost:8083/metrics
curl http://localhost:8082/metrics
```

## HTTP API

所有服务暴露以下端点：

### GET /health

健康检查

**响应示例:**
```json
{
  "status": "healthy",
  "service": "s2e",
  "version": "0.1.0",
  "uptime": 3600,
  "timestamp": "2025-11-17T12:00:00Z"
}
```

### GET /status

服务状态查询

**响应示例:**
```json
{
  "service": "s2e",
  "listening": true,
  "source_chain": {
    "name": "1024chain",
    "chain_id": 91024,
    "rpc": "https://rpc-testnet.1024chain.com/rpc/",
    "connected": true,
    "last_block": 1234567
  },
  "target_chain": {
    "name": "Arbitrum Sepolia",
    "chain_id": 421614,
    "rpc": "https://sepolia-rollup.arbitrum.io/rpc",
    "connected": true,
    "last_block": 7654321
  },
  "relayer": {
    "address": "0x1234...5678",
    "whitelisted": true,
    "balance_svm": 10.5,
    "balance_evm": 0.5
  }
}
```

### GET /metrics

Prometheus 格式指标

**响应示例:**
```
# HELP relayer_events_total Total number of events processed
# TYPE relayer_events_total counter
relayer_events_total{service="s2e",status="success"} 1234
relayer_events_total{service="s2e",status="failed"} 3

# HELP relayer_balance_svm SVM balance in SOL
# TYPE relayer_balance_svm gauge
relayer_balance_svm 10.5
```

## Docker 部署

### 1. 准备配置文件

```bash
# 创建目录
mkdir -p logs/s2e logs/e2s-listener logs/e2s-submitter queue keys

# 复制配置文件
cp s2e/config.example.env s2e/.env
# 编辑 s2e/.env 填入实际配置

# 复制密钥
cp /path/to/your/svm-wallet.json keys/
chmod 400 keys/svm-wallet.json
```

### 2. 构建并启动

```bash
# 构建镜像
docker-compose build

# 启动服务
docker-compose up -d

# 查看状态
docker-compose ps

# 查看日志
docker-compose logs -f
```

### 3. 验证服务

```bash
# 检查健康状态
curl http://localhost:8083/health  # s2e
curl http://localhost:8082/health  # e2s-submitter

# 查看日志
docker-compose logs s2e-relayer
docker-compose logs e2s-listener
docker-compose logs e2s-submitter
```

### Docker Compose 服务

| 服务 | 端口 | 说明 |
|------|------|------|
| s2e-relayer | 8083 | SVM→EVM 中继服务 |
| e2s-listener | - | EVM 事件监听器（无对外端口） |
| e2s-submitter | 8082 | SVM 交易提交器 + API |

### 数据持久化

```
relayer1/
├── logs/              # 日志文件（挂载）
│   ├── s2e/
│   ├── e2s-listener/
│   └── e2s-submitter/
├── queue/             # 事件队列（listener与submitter共享）
└── keys/              # 密钥文件（只读挂载）
```

## 配置说明

### 配置格式

使用双下划线 `__` 分隔层级：

```bash
SECTION__KEY=value
```

### 通用配置项

| 配置项 | 说明 | 示例 |
|--------|------|------|
| `SERVICE__NAME` | 服务名称 | `s2e` |
| `SERVICE__VERSION` | 版本号 | `0.1.0` |
| `SERVICE__WORKER_POOL_SIZE` | Worker 数量 | `5` |
| `API__PORT` | API 端口 | `8083` |
| `LOGGING__LEVEL` | 日志级别 | `info` / `debug` / `warn` / `error` |
| `LOGGING__FORMAT` | 日志格式 | `text` / `json` |

### 链配置

| 配置项 | 说明 |
|--------|------|
| `SOURCE_CHAIN__NAME` | 源链名称 |
| `SOURCE_CHAIN__CHAIN_ID` | 源链 ID |
| `SOURCE_CHAIN__RPC_URL` | 源链 RPC 地址 |
| `SOURCE_CHAIN__CONTRACT_ADDRESS` | 源链合约地址 |
| `TARGET_CHAIN__NAME` | 目标链名称 |
| `TARGET_CHAIN__CHAIN_ID` | 目标链 ID |
| `TARGET_CHAIN__RPC_URL` | 目标链 RPC 地址 |
| `TARGET_CHAIN__CONTRACT_ADDRESS` | 目标链合约地址 |

### 密钥配置

**S2E 需要:**
- `RELAYER__ECDSA_PRIVATE_KEY`: ECDSA 私钥（用于 EVM 签名）

**E2S 需要:**
- `RELAYER__ED25519_PRIVATE_KEY`: Ed25519 私钥（用于 SVM 签名和交易）

### 队列配置

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `QUEUE__PATH` | `.relayer/queue` | 队列目录路径 |
| `QUEUE__MAX_SIZE` | `1000` | 最大队列大小 |
| `QUEUE__RETRY_LIMIT` | `5` | 最大重试次数 |
| `QUEUE__RETRY_DELAYS` | `0,30000,60000,120000,300000` | 重试延迟(毫秒) |

### Gas 配置

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `GAS__MIN_SVM_BALANCE` | `5.0` | 最低 SVM 余额 (SOL) |
| `GAS__MIN_EVM_BALANCE` | `0.1` | 最低 EVM 余额 (ETH) |
| `GAS__BALANCE_CHECK_INTERVAL` | `300000` | 余额检查间隔 (毫秒) |

## 密钥管理

### S2E 服务密钥

```bash
# ECDSA 私钥（用于 EVM 签名）
RELAYER__ECDSA_PRIVATE_KEY=0x1234567890abcdef...
```

### E2S 服务密钥

```bash
# Ed25519 私钥（用于 SVM 签名和交易）
RELAYER__ED25519_PRIVATE_KEY=base58-encoded-key...

# 或使用 Solana 钱包文件
RELAYER__SVM_WALLET_PATH=/path/to/wallet.json
```

### 安全建议

- ✅ 使用环境变量或密钥管理服务（AWS KMS、HashiCorp Vault）
- ✅ 生产环境禁止硬编码密钥
- ✅ 定期轮换密钥
- ✅ 使用不同的密钥对用于测试和生产环境
- ❌ 禁止将私钥提交到代码仓库
- ❌ 禁止在日志中打印私钥

## 日志系统

### 日志级别

| 级别 | 用途 |
|------|------|
| `debug` | 详细调试信息 |
| `info` | 正常流程信息 |
| `warn` | 警告但不影响运行 |
| `error` | 错误需要关注 |

### 日志格式

- **text**: 可读文本格式（开发环境）
- **json**: 结构化 JSON 格式（生产环境，便于日志聚合）

### 日志输出

```bash
# 控制台输出（默认）
cargo run --release

# 重定向到文件
cargo run --release 2>&1 | tee relayer.log

# Docker 日志
docker-compose logs -f s2e-relayer
```

## 监控和指标

### Prometheus 指标

```bash
# 查看所有指标
curl http://localhost:8083/metrics
```

**主要指标:**

- `relayer_events_total`: 处理的事件总数
- `relayer_events_success`: 成功的事件数
- `relayer_events_failed`: 失败的事件数
- `relayer_balance_svm`: SVM 余额 (SOL)
- `relayer_balance_evm`: EVM 余额 (ETH)
- `relayer_uptime_seconds`: 运行时间 (秒)

### 健康检查

```bash
# 简单检查
curl http://localhost:8083/health

# 详细状态
curl http://localhost:8083/status | jq
```

## 故障排查

### 服务无法启动

**症状**: 服务启动后立即退出

**检查步骤**:
```bash
# 1. 检查配置文件
cat .env

# 2. 检查日志
cargo run --release

# 3. 验证 RPC 连接
curl $SOURCE_CHAIN__RPC_URL
curl $TARGET_CHAIN__RPC_URL
```

**常见原因**:
- 配置文件缺少必需字段
- RPC 地址无法访问
- 私钥格式错误

### RPC 连接失败

**症状**: `sourceChain.connected: false`

**解决方案**:
- 检查网络连接
- 验证 RPC URL 配置
- 检查防火墙规则
- 尝试使用备用 RPC 节点

### 签名验证失败

**症状**: `Transaction reverted: Invalid signature`

**可能原因**:
- 密钥配置错误（混淆了 Ed25519 和 ECDSA）
- 事件数据序列化错误
- 哈希算法不匹配

**调试步骤**:
```bash
# 查看详细日志
RUST_LOG=debug cargo run --release

# 检查配置
curl http://localhost:8083/config
```

### Gas 不足

**症状**: `Insufficient funds for gas`

**解决方案**:
```bash
# 检查余额
curl http://localhost:8083/status | jq '.relayer.balance_svm'
curl http://localhost:8083/status | jq '.relayer.balance_evm'

# 为钱包充值
# SVM: 至少 5 SOL
# EVM: 至少 0.1 ETH
```

### E2S 队列堆积

**症状**: 队列文件数量持续增加

**检查步骤**:
```bash
# 查看队列
ls -lh .relayer/queue/

# 检查 submitter 日志
docker-compose logs e2s-submitter

# 检查 submitter 是否运行
curl http://localhost:8082/health
```

**解决方案**:
- 检查 SVM RPC 连接
- 确认 relayer 已注册白名单
- 检查 relayer 余额是否充足

## 技术栈

### 核心依赖

| 依赖 | 版本 | 用途 |
|------|------|------|
| tokio | 1.35 | 异步运行时 |
| axum | 0.7 | HTTP 框架 |
| ethers | 2.0.14 | EVM 客户端 |
| solana-sdk | - | SVM 客户端 (仅 e2s-submitter) |
| secp256k1 | 0.28 | ECDSA 签名 (s2e) |
| ed25519-dalek | - | Ed25519 签名 (e2s) |
| serde/serde_json | 1.0 | 序列化 |
| tracing | 0.1 | 日志系统 |
| anyhow/thiserror | 1.0 | 错误处理 |

### 为什么选择 Rust?

- ✅ 高性能、低内存占用
- ✅ 强类型系统，减少运行时错误
- ✅ 优秀的异步编程支持 (Tokio)
- ✅ 丰富的区块链库生态 (ethers, solana-sdk)
- ✅ 内存安全，无 GC 停顿

## 开发指南

### 本地开发

```bash
# 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 克隆代码
cd relayer1

# 编译
cargo build

# 运行测试
cargo test

# 运行（开发模式）
cargo run

# 运行（发布模式）
cargo run --release
```

### 代码检查

```bash
# 格式化代码
cargo fmt

# 静态检查
cargo clippy -- -D warnings

# 运行测试
cargo test --all
```

### 添加新功能

1. 在相应模块添加代码
2. 更新 `shared/src/types.rs` 如果需要新类型
3. 运行测试确保不破坏现有功能
4. 更新文档

## 性能优化建议

### 1. RPC 连接池

当前实现为每个请求创建新连接，生产环境建议使用连接池：

```rust
// 使用 r2d2 或类似连接池库
let pool = ConnectionPool::new(config.rpc_url, 10)?;
```

### 2. 批量处理

如果合约支持批量提交签名：

```rust
// 批量收集事件
let events = collect_events().await?;

// 批量生成签名
let signatures = events.iter()
    .map(|e| sign_event(e))
    .collect::<Vec<_>>();

// 批量提交
submit_batch(signatures).await?;
```

### 3. 缓存

缓存已处理的 nonce，避免重复查询：

```rust
use std::collections::HashSet;
let processed_nonces: HashSet<u64> = HashSet::new();

if processed_nonces.contains(&nonce) {
    return; // 跳过已处理的
}
```

## 安全注意事项

### 1. 密钥保护

- ✅ 使用环境变量或密钥管理服务
- ✅ 文件权限设置为 400 (只读，仅所有者)
- ✅ 不要在日志中打印私钥
- ✅ 使用不同的密钥用于开发和生产

### 2. 网络安全

- ✅ 使用 HTTPS RPC 端点
- ✅ 启用 API 认证（生产环境）
- ✅ 配置防火墙规则
- ✅ 使用私有 RPC 节点（推荐）

### 3. 运行时安全

- ✅ 使用非特权用户运行服务
- ✅ 设置资源限制（内存、CPU）
- ✅ 启用健康检查和自动重启
- ✅ 定期备份配置和密钥

### 4. 代码安全

- ✅ 定期更新依赖
- ✅ 运行安全审计: `cargo audit`
- ✅ 使用 `clippy` 检查代码质量
- ✅ 编写单元测试和集成测试
