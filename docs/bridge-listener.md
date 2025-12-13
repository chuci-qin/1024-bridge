# Bridge CrossChainSuccess 监听服务使用指南

## 概述

Bridge CrossChainSuccess 监听服务是一个自动化的跨链入金服务，用于监听 Bridge Program 的 `CrossChainSuccess` 事件，并自动调用 Vault Program 完成用户入金。

## 架构（2025-12-13 更新）

```
EVM 链 (Arbitrum)
  ↓ 用户转 USDC (MetaMask: 0xd4B42...)
Bridge 合约
  ↓ emit StakeEvent {
      sender: 0xd4B42... (EVM 发起者)
      receiverAddress: "xxx" (可选的 Solana 地址)
    }
e2s-submitter
  ↓ 2/3 多签验证
  ↓ emit CrossChainSuccessEvent {
      evm_address: 0xd4B42... (来自 sender)
    }
BridgeListener (监听服务)
  ↓ 解析 EVM 地址
  ↓ 映射到 Solana 地址 (CxDgkz...)
  ↓ HTTP POST /api/testnet/deposit
Frontend API
  ↓ 调用 Vault.RelayerDeposit
目标用户 Vault 账本 +N USDC ✅
```

**架构变更**（2025-12-13）：
- ✅ 不再使用中转账户
- ✅ 不再直接调用链上指令
- ✅ 改为 HTTP 调用 Frontend deposit API
- ✅ StakeEvent 新增 `sender` 字段存储 EVM 发起者
- ✅ CrossChainSuccessEvent.evm_address 来自 `sender` 而不是 `receiver_address`

## 功能特性

- ✅ **自动化**: 无需手动触发，事件驱动自动入金
- ✅ **EVM 地址映射**: 支持 EVM 钱包跨链到 1024Chain（使用确定性映射算法）
- ✅ **智能地址检测**: 自动识别 EVM (0x...) 和 Solana (Base58) 地址格式
- ✅ **防重放保护**: 基于 nonce 持久化存储，重启后不会重复处理
- ✅ **HTTP API 调用**: 通过 Frontend deposit API 完成入金，架构简单
- ✅ **容错设计**: 网络异常自动重试，详细错误日志
- ✅ **详细日志**: 完整的操作日志，便于审计和调试

## 前置准备

### 1. 准备管理员密钥

Bridge Listener 使用管理员账户（通过 HTTP API）完成入金操作。

```bash
# 管理员密钥文件（如 faucet.json）
export RELAY_KEYPAIR_PATH="./faucet.json"
```

**注意**: Frontend deposit API 需要配置相同的管理员私钥（环境变量 `TESTNET_ADMIN_PRIVATE_KEY`）

### 2. 部署 Bridge Program

确保 Bridge Program 包含最新的 `CrossChainSuccessEvent` 定义：

```bash
cd 1024-bridge/svm/bridge1024
anchor build
anchor deploy
```

**重要**: 确保合约版本包含以下修复：
- ✅ StakeEventData 包含 `sender` 字段（EVM 发起者地址）
- ✅ CrossChainSuccessEvent.evm_address 使用 `event_data.sender`

### 3. 启动 Frontend 服务

Bridge Listener 通过 HTTP 调用 Frontend deposit API：

```bash
cd 1024-chain-frontend
npm run dev
# Frontend 将运行在 http://localhost:3000
```

## 配置（2025-12-13 更新）

### 环境变量配置

```bash
cd 1024-bridge
cp bridge-listener.env.example bridge-listener.env
```

编辑 `bridge-listener.env`：

```bash
# Solana RPC 地址
SOLANA_RPC_URL=https://testnet-rpc.1024chain.com/rpc/

# Bridge Program ID（最新部署）
BRIDGE_PROGRAM_ID=FzyXa3DjKM29D7W6bhHJeb2wMSiHPKsJaEZsnvKt3dBR

# Vault Program ID（V2 记账模式）
VAULT_PROGRAM_ID=vR3BifKCa2TGKP2uhToxZAMYAYydqpesvKGX54gzFny

# 管理员密钥文件路径（用于通过 API 调用）
RELAY_KEYPAIR_PATH=./faucet.json

# Frontend API 地址（新增）
FRONTEND_API_URL=http://localhost:3000

# 日志级别（可选）
RUST_LOG=info,bridge_listener=debug
```

## 启动服务

### 方式 1: 使用启动脚本

```bash
cd 1024-bridge
./start-bridge-listener.sh
```

### 方式 2: 手动启动

```bash
# 加载环境变量
export $(grep -v '^#' bridge-listener.env | xargs)

# 启动服务
cd ../1024-core
cargo run --release --bin bridge-listener
```

## 运行日志

正常运行时的日志示例：

```
🌉 启动 Bridge CrossChainSuccess 监听服务
========================================
🌉 BridgeListener 初始化:
   RPC: https://testnet-rpc.1024chain.com/rpc/
   Bridge Program: FzyXa3DjKM29D7W6bhHJeb2wMSiHPKsJaEZsnvKt3dBR
   Vault Program: vR3BifKCa2TGKP2uhToxZAMYAYydqpesvKGX54gzFny
   Relay Keypair: ./faucet.json
   Frontend API: http://localhost:3000
✅ 中转账户: 267TEwwHkJUHz42TLNggDCecNhYHFxcRALmR17bPkvU8
✅ VaultConfig PDA: rMLrkwxV4uNLKmL2vmP3CJbYPbKamjZD4wjeKZsCy1g

🚀 启动 Bridge CrossChainSuccess 事件监听服务
🔍 正在查询 Bridge Program 最新交易...
📋 发现 10 个最近交易

🎉🎉🎉 收到 CrossChainSuccess 事件！
📋 事件详情:
   EVM 地址: 0xd4B42EfF8AF8eF82dE3830fE30559bfF92Dca55F
   金额: 100 USDC
   Nonce: 42
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🔄 EVM 地址映射:
   EVM: 0xd4B42EfF8AF8eF82dE3830fE30559bfF92Dca55F
   Solana: CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC
📤 调用 deposit API:
   URL: http://localhost:3000/api/testnet/deposit
   钱包: CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC
   金额: 100 USDC
✅ 入金成功!
   交易哈希: 5wfVu6wRFsLEGCt4EotxjqSsVcdFy5sKChkgXtTD8eqD...
   🔗 https://testnet-scan.1024chain.com/tx/5wfVu6...
✅ 成功处理 CrossChainSuccess 事件 (Nonce: 42)
```

## 测试

### 端到端测试流程

1. **从 Arbitrum 发起转账**

```bash
# 在 Arbitrum 上转账 100 USDC
# 接收地址填写 EVM 地址
npx ts-node scripts/evm-user-stake.ts \
  --amount 100 \
  --receiver 0x742d35Cc6634C0532925a3b844Bc9e7595f0bEbA
```

2. **等待 e2s-submitter 处理**

e2s-submitter 会自动监听 StakeEvent 并提交签名。当达到 2/3 多签时，Bridge 合约会发出 `CrossChainSuccess` 事件。

3. **查看 BridgeListener 日志**

BridgeListener 会自动监听到事件并处理：

```
🎉 收到 CrossChainSuccess 事件
🔄 EVM 地址映射: 0x742d... → 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU
✅ 成功处理 CrossChainSuccess 事件
```

4. **验证用户余额**

```bash
# 查询目标用户 Vault 余额
# EVM 地址: 0x742d35Cc6634C0532925a3b844Bc9e7595f0bEbA
# Solana 地址: 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU

curl -X GET "http://localhost:8082/api/v1/account/balance?wallet=7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"

# 应返回: { "balance_e6": 100000000 } (100 USDC)
```

### 防重放测试

1. 重启 BridgeListener 服务
2. 确认不会重复处理已处理的 nonce

预期日志：

```
⏭️  Nonce 42 已处理，跳过
```

## 监控和维护

### 健康检查

```bash
# 检查服务是否运行
ps aux | grep bridge-listener

# 查看服务日志
tail -f logs/bridge-listener.log
```

### 常见问题

#### 1. 服务启动失败

**错误**: `RELAY_KEYPAIR_PATH environment variable not set`

**解决**: 检查 `bridge-listener.env` 配置文件是否正确加载。

#### 2. 中转账户权限不足

**错误**: `Unauthorized: RelayTransit not in authorized_callers`

**解决**: 确保中转账户已注册到 Vault 的 `authorized_callers` 列表。

#### 3. 事件未被监听到

**可能原因**:
- Bridge Program 未正确发出 `CrossChainSuccess` 事件
- RPC 节点延迟
- 轮询间隔过长

**解决**:
- 检查 Bridge 合约日志
- 增加日志级别: `RUST_LOG=debug`
- 调整轮询间隔（修改 `POLLING_INTERVAL_SECS` 常量）

#### 4. EVM 地址映射错误

**错误**: 目标用户收不到入金

**解决**: 
- 确认 EVM 地址格式正确（0x 前缀）
- 验证映射算法与 `account-domain/mapping.rs` 一致
- 使用在线工具验证映射: `1024-exchange-evm:{address}` → SHA256 → Base58

## 技术细节

### EVM 地址映射算法

```rust
fn derive_solana_address_from_evm(evm_address: &str) -> String {
    // 1. 标准化 (小写，去 0x)
    let normalized = evm_address.to_lowercase().replace("0x", "");
    
    // 2. 添加域前缀
    let prefixed = format!("1024-exchange-evm:{}", normalized);
    
    // 3. SHA256 哈希
    let hash = Sha256::digest(prefixed.as_bytes());
    
    // 4. Base58 编码
    bs58::encode(&hash[..]).into_string()
}
```

**特点**:
- ✅ 确定性：相同 EVM 地址总是映射到相同 Solana 地址
- ✅ 唯一性：不同 EVM 地址映射到不同 Solana 地址
- ✅ 可验证：任何人都可以独立验证映射关系

### 事件解析

Anchor 事件格式：

```
Program data: <base64-encoded>
  ↓
[discriminator(8 bytes)] + [event_data]
  ↓
CrossChainSuccessEvent {
    evm_address: String,
    amount: u64,
    nonce: u64,
    source_chain_id: u64,
    block_height: u64,
}
```

### 防重放机制

使用内存缓存 nonce 去重：

```rust
let processed_nonces: Arc<Mutex<HashSet<u64>>>;

// 检查是否已处理
if processed_nonces.contains(&event.nonce) {
    return Ok(());  // 跳过
}

// 标记为已处理
processed_nonces.insert(event.nonce);
```

## 高可用部署

### 使用 systemd

创建服务文件 `/etc/systemd/system/bridge-listener.service`：

```ini
[Unit]
Description=Bridge CrossChainSuccess Listener Service
After=network.target

[Service]
Type=simple
User=solana
WorkingDirectory=/opt/1024ex/1024-bridge
EnvironmentFile=/opt/1024ex/1024-bridge/bridge-listener.env
ExecStart=/opt/1024ex/1024-core/target/release/bridge-listener
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

启动服务：

```bash
sudo systemctl daemon-reload
sudo systemctl enable bridge-listener
sudo systemctl start bridge-listener
sudo systemctl status bridge-listener
```

### 日志收集

使用 journalctl 查看日志：

```bash
journalctl -u bridge-listener -f
```

或配置日志文件：

```bash
# 在启动脚本中重定向日志
cargo run --release --bin bridge-listener >> logs/bridge-listener.log 2>&1
```

## 总结

Bridge CrossChainSuccess 监听服务提供了一个自动化、可靠、易于维护的跨链入金解决方案。通过事件驱动架构，实现了 EVM 钱包与 1024Chain Vault 系统的无缝集成。

---

**最后更新**: 2025-12-11  
**维护人员**: 跨链桥开发团队



