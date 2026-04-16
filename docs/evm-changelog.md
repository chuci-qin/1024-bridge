# Bridge1024 EVM 合约重构记录

## 版本概述

本次 EVM 合约（`contracts/evm/src/Bridge1024.sol`）经历了全面重构，核心目标：**安全性增强、结构轻量化、gas 优化**。

---

## 一、安全性增强

### 1.1 投票确认机制（替代「第一人定调」模式）

旧设计：第一个 relayer 提交的 `eventData` 作为基准，后续 relayer 必须完全一致，否则 revert。如果第一个 relayer 提交了错误数据，该 nonce 永久卡死。

新设计：每个 relayer 提交的 `eventData` 取 `keccak256` 哈希后独立投票，`hashVotes[dataHash]++`。当某个哈希的票数达到 `ceil(2/3)` 阈值时触发 unlock，使用当前内存中的 `eventData` 执行转账。

**优势：**
- 少数 relayer 提交错误数据不影响正常流程
- 无需在 storage 中保存完整 `eventData`，只保存哈希投票计数
- 无 nonce 卡死风险

### 1.2 ECDSA 签名验证移除

在「每个 relayer 独立提交到链上」的模型中，`msg.sender` 已经通过 `onlyWhitelistedRelayer` 验证了身份。合约内再做一次 ECDSA 签名验证是冗余的——安全性等价，但额外消耗 ~6000 gas。

移除后，`confirmEvent` 不再需要 `signature` 参数，删除了 `_verifyEcdsaSignature`、`_hashEventData`、`_bytes32ToHex`、`_uint64ToString` 等内部函数，以及 `SECP256K1_N_HALF` 常量。

### 1.3 Guardian 紧急暂停角色

新增 `guardian` 地址（EOA），仅有 `pause()` 权限，不能 `unpause()` 或执行其他管理操作。

设计意图：`admin` 使用多签钱包保障安全性（签名延迟较高），`guardian` 使用 EOA 保障紧急响应速度。即使 `guardian` 私钥泄露，攻击者只能暂停合约（造成 DoS），无法盗取资金或恢复运行。

### 1.3.1 Operator 运维角色

新增 `operator` 地址（EOA），负责日常运维操作：`skipNonce` 和 `refund`。

设计意图：将故障处理权限从 `admin`（多签钱包）分离，提高响应速度。`operator` 的权限有界——只能退还已 stake 的金额（链上记录），不能动金库，`admin` 可随时更换 `operator`。即使 `operator` 私钥泄露，攻击者只能退款（不可重复），无法盗取超出已锁定范围的资金。

### 1.4 目标链验证

`confirmEvent` 新增两项检查：
- `eventData.targetChainId == localChainId`
- `eventData.targetContract == address(this)`

防止恶意 relayer 将 A 链的事件提交到 B 链的合约上。

### 1.5 receiver 零地址防护

`confirmEvent` 解锁前检查解析后的 `receiver != address(0)`，防止代币被销毁（OpenZeppelin ERC20 v5 的 `transfer(address(0), amount)` 不会 revert，而是直接销毁）。

### 1.6 amount 零值检查

`confirmEvent` 入口处检查 `eventData.amount != 0`，避免无意义的零金额操作浪费 gas。

### 1.7 管理操作事件审计

所有关键管理操作新增事件，便于链上审计和监控：
- `configure()` → `BridgeConfigured`
- `configureRateLimits()` → `RateLimitsConfigured`
- `emergencyWithdraw()` → `EmergencyWithdrawal`

### 1.8 configure 运行期修改警告

`configure()` 函数注释中明确标注：在合约正常运行期间修改 `peerContract` 或链 ID，会导致所有进行中的 `NonceConfirmation` 因校验不匹配而永久卡住。建议先 `pause()` 再修改。

### 1.9 细化错误类型

- `InvalidChainId` → `InvalidSourceChainId` + `InvalidTargetChainId`
- `InvalidSourceContract` 保留，新增 `InvalidTargetContract`

更精确的 revert 信息加速问题定位。

### 1.10 Nonce 故障挽救机制（skipNonce + refund）

新增两个 admin 函数，用于处理 unlock 永久失败的 nonce：

- **接收端** `skipNonce(nonce)`：将 nonce 标记为已处理（`processedNonces[nonce] = true`），永久封死 unlock 可能
- **发送端** `refund(nonce, to)`：从链上 `stakeAmounts[nonce]` 读取金额，退还到 operator 指定的地址

**操作流程**：先在接收端 `skipNonce`，再在发送端 `refund`。顺序不可颠倒，否则存在双花风险。

**设计选择**：
- `stake()` 将金额记录到 `stakeAmounts[nonce]`，`refund()` 从链上验证金额，避免 operator 篡改退款金额
- 不存储 sender 地址，`refund` 的 `to` 参数由 operator 指定，允许用户选择退到不同地址
- `skipNonce` 和 `refund` 由 `operator` 角色执行，而非 `admin`，权限分离更安全

---

## 二、结构轻量化

### 2.1 SenderConfig / ReceiverConfig 结构体移除

旧设计：
```solidity
struct SenderConfig { uint64 nonce; }
struct ReceiverConfig { uint64 relayerCount; address[] relayers; }
SenderConfig public senderConfig;
ReceiverConfig internal receiverConfig;
```

新设计：
```solidity
uint64 public senderNonce;
address[] public relayers;
```

每个 struct 只剩一个字段，包装层无意义。直接暴露为顶层状态变量，代码更直观。

### 2.2 relayerCount 冗余字段移除

`relayerCount` 与 `relayers.length` 完全等价，每次 add/remove 都要额外 SSTORE 同步。删除后所有读取改用 `relayers.length`（同样是 1 次 SLOAD），消除一致性风险。

### 2.3 decimalRatio 移除

所有 USDC 均为 6 位精度，`decimalRatio` 始终为 1，引入不必要的除法和乘法操作。移除后 `stake` 和 `confirmEvent` 中的金额处理更简洁。

### 2.4 SharedState 合并

原 `SenderState` 和 `ReceiverState` 有大量重复字段（admin、chainId、peerContract、usdcContract 等）。合并为 `SharedState`，字段排列经过优化以实现最佳 storage packing：

```solidity
struct SharedState {
    address admin;        // 20B
    uint64 localChainId;  //  8B  → slot 0 (28B)
    address usdcContract; // 20B
    uint64 peerChainId;   //  8B  → slot 1 (28B)
    address pendingAdmin; // 20B  → slot 2
    bytes32 peerContract; // 32B  → slot 3
}
```

### 2.5 NonceConfirmation 精简

旧设计（7 个字段）：
```solidity
struct NonceConfirmation {
    mapping(address => bool) confirmedRelayers;
    uint8 confirmCount;
    bool isUnlocked;
    bool isInitialized;
    uint8 frozenThreshold;
    StakeEventData eventData;  // 完整事件数据存储在 storage
}
```

新设计（4 个字段）：
```solidity
struct NonceConfirmation {
    mapping(address => bool) confirmedRelayers;
    mapping(bytes32 => uint8) hashVotes;
    bool isUnlocked;
    uint8 frozenThreshold;
}
```

- 删除 `confirmCount`（被 `hashVotes` 取代）
- 删除 `isInitialized`（用 `frozenThreshold == 0` 判断）
- 删除 `eventData`（投票模式下无需存储，达到阈值时使用内存中的数据）

### 2.6 receiverAddress string → bytes32

`StakeEventData.receiverAddress` 从 `string` 改为 `bytes32 receiver`，实现全定长结构：
- EVM 地址：20 字节右对齐存入 bytes32（`bytes32(uint256(uint160(addr)))`）
- SVM Pubkey：原生 32 字节直接存入

**删除的函数和错误**：
- `_parseAddress` — hex 字符串解析为 address
- `_validateReceiverAddress` — 字符串字符合法性校验
- `InvalidReceiverAddress` — 不再需要

`stake()` 参数相应改为 `bytes32 receiver`，仅校验 `!= bytes32(0)`。
`confirmEvent()` 中用 `address(uint160(uint256(eventData.receiver)))` 直接提取 EVM 地址。

### 2.7 内部函数清理

移除了不再需要的函数：
- `_verifyEcdsaSignature` — 签名验证
- `_hashEventData` — EIP-191 哈希
- `_bytes32ToHex` — 序列化辅助
- `_uint64ToString` — 序列化辅助
- `_verifyEventDataConsistency` — 逐字段比较（被投票哈希取代）
- `_parseAddress` — hex 字符串→address 解析
- `_validateReceiverAddress` — 字符串合法性校验

---

## 三、Gas 优化

| 操作 | 优化点 | 节省 |
|------|--------|------|
| `confirmEvent` | 移除 ECDSA 验证（ecrecover ~3000 gas + 辅助函数） | ~6000 gas/次 |
| `confirmEvent` | 移除 `eventData` storage 写入（9 个字段 × SSTORE） | ~40000+ gas（首次确认） |
| `confirmEvent` | 投票哈希 vs 逐字段比较（1 次 keccak256 vs 9 次 SLOAD+比较） | ~2000 gas（后续确认） |
| `confirmEvent` | 重复确认检查提前（RelayerAlreadyConfirmed 在重计算前 revert） | 重复提交时省 ~56% gas |
| `addRelayer` | 移除 `relayerCount` SSTORE 同步 | ~5000 gas/次 |
| `removeRelayer` | 移除 `relayerCount` SSTORE 同步 | ~5000 gas/次 |
| `stake` | 移除 `decimalRatio` 除法 | ~30 gas/次 |
| `stake` | `receiverAddress` string→bytes32，免去字符串校验循环 | ~500+ gas/次 |
| `confirmEvent` | `receiver` bytes32 直接 cast，免去 hex 解析循环 | ~2000+ gas/次 |
| `confirmEvent` | 全定长 `abi.encode`，hash 更高效 | ~200 gas/次 |
| 部署 | 移除 7 个内部函数 + 1 个常量 + 1 个错误，减少合约字节码 | ~3000+ 部署 gas |

---

## 四、测试覆盖

54 个 Foundry 测试全部通过，覆盖：

- **初始化**：admin 设置、configure 参数校验
- **Stake**：正常流程、余额不足、未授权、暂停状态、零地址接收者
- **投票确认**：单 relayer、阈值达标、nonce 乱序、重放防护、非白名单、源/目标合约链 ID 校验
- **投票机制**：少数错误数据不影响、无共识不解锁、篡改金额不影响正确多数
- **速率限制**：窗口超限、滑动窗口衰减、单笔超限、最低储备金
- **管理员**：提议/接受、权限控制、暂停/恢复、guardian 权限、operator 权限、emergencyWithdraw
- **故障挽救**：skipNonce 封死 unlock、refund 退款、退到不同地址、重复退款防护、参数校验、operator 权限控制
- **安全**：重放攻击、零金额确认、零地址接收者、全面权限控制

---

## 五、完整功能列表

| 函数 | 权限 | 说明 |
|------|------|------|
| `configure` | admin | 设置 USDC 地址、对端合约、链 ID |
| `configureRateLimits` | admin | 设置速率限制参数 |
| `addRelayer` | admin | 添加中继者 |
| `removeRelayer` | admin | 移除中继者 |
| `rotateRelayer` | admin | 原子替换中继者 |
| `proposeAdmin` | admin | 发起两步管理员转移 |
| `acceptAdmin` | pendingAdmin | 接受管理员转移 |
| `setGuardian` | admin | 设置/移除 guardian |
| `setOperator` | admin | 设置/移除 operator |
| `pause` | admin / guardian | 紧急暂停 |
| `unpause` | admin | 恢复运行 |
| `emergencyWithdraw` | admin | 紧急提取代币 |
| `skipNonce` | operator | 跳过 nonce，封死 unlock（接收端） |
| `refund` | operator | 退款锁定资金（发送端） |
| `stake` | 任何人 | 锁定 USDC 发起跨链 |
| `confirmEvent` | relayer | 投票确认跨链事件 |
| `getRelayerCount` | view | 查询中继者数量 |
| `isRelayer` | view | 查询是否为中继者 |
