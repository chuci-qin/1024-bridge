# Bridge1024 API Reference

## EVM Contract API

### Write Functions

#### `stake(uint256 amount, bytes32 receiver) → uint64`
用户锁定 USDC 发起跨链转移，返回 nonce。
- `amount`: USDC 金额（6 位精度原始单位）
- `receiver`: 目标链接收者地址（EVM 地址右对齐 20B 存入 bytes32，SVM Pubkey 原生 32B 直接存入）
- 权限：任何人（需 USDC approve）
- 修饰器：`whenNotPaused`, `nonReentrant`
- 事件：`StakeEvent`

#### `confirmEvent(StakeEventData eventData)`
中继者投票确认跨链事件。同一 `eventData` 哈希的票数达到 `ceil(2/3)` 阈值后自动解锁 USDC。
- `eventData`: 完整的跨链事件数据结构体
- 权限：白名单 relayer
- 修饰器：`onlyWhitelistedRelayer`, `whenNotPaused`, `nonReentrant`
- 事件：`EventConfirmed`（每次投票）、`TokensUnlocked`（达到阈值时）

#### `configure(address usdcAddress, bytes32 peerContract, uint64 localChainId, uint64 peerChainId)`
一次性配置桥的核心参数。
- 权限：admin
- 事件：`BridgeConfigured`
- ⚠️ 运行中修改会导致进行中的确认卡住，建议先 `pause()`

#### `configureRateLimits(uint256 maxPerWindow, uint256 windowDuration, uint256 maxSingle, uint256 minReserve)`
配置速率限制参数。
- `maxPerWindow`: 每个时间窗口最大解锁总额（0 = 不限制）
- `windowDuration`: 窗口持续时长（秒）
- `maxSingle`: 单笔最大解锁金额（0 = 不限制）
- `minReserve`: 金库最低储备金
- 权限：admin
- 事件：`RateLimitsConfigured`

#### `addRelayer(address)` / `removeRelayer(address)` / `rotateRelayer(address old, address new)`
管理中继者白名单（上限 18 个）。
- 权限：admin
- 事件：`RelayerAdded` / `RelayerRemoved`

#### `proposeAdmin(address newAdmin)`
发起两步管理员转移（第一步：提议）。
- 权限：admin
- 事件：`AdminTransferProposed`

#### `acceptAdmin()`
完成两步管理员转移（第二步：接受）。
- 权限：pendingAdmin
- 事件：`AdminTransferAccepted`

#### `setGuardian(address _guardian)`
设置/移除 guardian（传 `address(0)` 移除）。
- 权限：admin
- 事件：`GuardianUpdated`

#### `setOperator(address _operator)`
设置/移除 operator（传 `address(0)` 移除）。
- 权限：admin
- 事件：`OperatorUpdated`

#### `pause()` / `unpause()`
紧急暂停/恢复。`pause` 可由 admin 或 guardian 调用，`unpause` 仅 admin。

#### `emergencyWithdraw(address token, uint256 amount, address to)`
紧急提取合约中的 ERC20 代币。
- 权限：admin
- 事件：`EmergencyWithdrawal`

#### `skipNonce(uint64 nonce)`
将某 nonce 永久标记为已处理，阻止后续 unlock（接收端使用）。
- 权限：operator
- 事件：`NonceSkipped`
- ⚠️ 必须在对端链 `refund` 之前调用

#### `refund(uint64 nonce, address to)`
退还某 nonce 锁定的资金（发送端使用）。金额从链上 `stakeAmounts[nonce]` 读取，退款地址由 operator 指定。
- 权限：operator
- 修饰器：`nonReentrant`
- 事件：`Refunded`
- ⚠️ 必须先在对端链 `skipNonce` 封死 unlock

### Read Functions

| 函数 | 返回值 | 说明 |
|------|--------|------|
| `admin()` | `address` | 管理员地址 |
| `localChainId()` | `uint64` | 本链 ID |
| `usdcContract()` | `address` | USDC 合约地址 |
| `peerChainId()` | `uint64` | 对端链 ID |
| `pendingAdmin()` | `address` | 待接受的新管理员 |
| `peerContract()` | `bytes32` | 对端桥合约地址 |
| `relayers(uint256 index)` | `address` | 按 index 查询中继者 |
| `getRelayerCount()` | `uint256` | 中继者总数 |
| `isRelayer(address)` | `bool` | 是否为白名单中继者 |
| `guardian()` | `address` | 当前 guardian |
| `operator()` | `address` | 当前 operator |
| `recovery()` | `address` | 当前 recovery |
| `stakes(uint64)` | `(address owner, uint64 amount, bool refunded)` | nonce 对应的 stake 记录 |
| `nonceConfirmations(uint64)` | `(bool isProcessed, bool isUnlocked, uint8 frozenThreshold)` | 确认进度 |
| `refundInitiatedAt(uint64)` | `uint64` | 退款发起时间戳（0 = 未发起） |
| `maxUnlockPerWindow()` | `uint64` | 窗口最大解锁额 |
| `windowDuration()` | `uint64` | 窗口时长 |
| `maxSingleUnlock()` | `uint64` | 单笔最大额 |
| `maxStakeAmount()` | `uint64` | 单笔最大 stake 额 |
| `minimumReserve()` | `uint64` | 最低储备金 |
| `getBridgeInfo()` | `(address admin, address guardian, address operator, address recovery, address pendingAdmin, address usdcContract, bytes32 peerContract, uint64 localChainId, uint64 peerChainId, bool paused, bool timelockActive, uint256 relayerCount)` | 聚合查询桥身份、配置和状态 |
| `getRateLimitStatus()` | `(uint64 maxUnlockPerWindow, uint64 windowDuration, uint64 maxSingleUnlock, uint64 maxStakeAmount, uint64 minimumReserve, uint64 currentWindowStart, uint64 currentWindowUsage, uint64 previousWindowUsage)` | 聚合查询速率限制配置和滑动窗口运行时状态 |

### Data Structures

#### `StakeEventData`
```solidity
struct StakeEventData {
    bytes32 sourceContract;   // 源链桥合约地址
    bytes32 targetContract;   // 目标链桥合约地址
    uint64  sourceChainId;    // 源链 ID
    uint64  targetChainId;    // 目标链 ID
    uint64  blockHeight;      // stake 区块高度
    uint64  amount;           // 金额（USDC 6 位精度）
    bytes32 sender;           // 发送者地址（右对齐 bytes32）
    bytes32 receiver;         // 接收者地址（EVM 右对齐 20B，SVM 原生 32B）
    uint64  nonce;            // 唯一事件编号
}
```

### Events

| 事件 | 触发时机 |
|------|----------|
| `StakeEvent(bytes32 indexed sourceContract, bytes32 indexed targetContract, ...)` | 用户 stake |
| `EventConfirmed(address indexed relayer, uint64 indexed nonce)` | relayer 投票 |
| `TokensUnlocked(uint64 indexed nonce, address receiver, uint64 amount, bytes32 sender)` | 解锁完成 |
| `BridgeConfigured(address indexed usdcContract, bytes32 peerContract, uint64 localChainId, uint64 peerChainId)` | configure 调用 |
| `RateLimitsConfigured(uint256, uint256, uint256, uint256)` | configureRateLimits 调用 |
| `EmergencyWithdrawal(address indexed token, address indexed to, uint256 amount)` | 紧急提取 |
| `NonceSkipped(uint64 indexed nonce)` | 跳过 nonce |
| `Refunded(uint64 indexed nonce, address indexed to, uint256 amount)` | 退款 |
| `RelayerAdded(address indexed relayer)` | 添加中继者 |
| `RelayerRemoved(address indexed relayer)` | 移除中继者 |
| `GuardianUpdated(address indexed oldGuardian, address indexed newGuardian)` | guardian 变更 |
| `OperatorUpdated(address indexed oldOperator, address indexed newOperator)` | operator 变更 |
| `AdminTransferProposed(address indexed currentAdmin, address indexed pendingAdmin)` | 管理员提议 |
| `AdminTransferAccepted(address indexed oldAdmin, address indexed newAdmin)` | 管理员接受 |

### Custom Errors

| 错误 | 说明 |
|------|------|
| `Unauthorized()` | 权限不足 |
| `ZeroAddress()` | 零地址 |
| `ZeroAmount()` | 零金额 |
| `UsdcNotConfigured()` | USDC 未配置 |
| `RelayerAlreadyExists()` | 中继者已存在 |
| `TooManyRelayers()` | 超出中继者上限 |
| `RelayerNotFound()` | 中继者不存在 |
| `AlreadyProcessed()` | nonce 已处理 |
| `InvalidSourceContract()` | 源合约不匹配 |
| `InvalidTargetContract()` | 目标合约不匹配 |
| `InvalidSourceChainId()` | 源链 ID 不匹配 |
| `InvalidTargetChainId()` | 目标链 ID 不匹配 |
| `RateLimitExceeded()` | 超出速率限制 |
| `SingleTransferExceeded()` | 超出单笔限额 |
| `InsufficientReserve()` | 储备金不足 |
| `RelayerAlreadyConfirmed()` | 重复确认 |
| `AlreadyRefunded()` | 重复退款 |

---

## SVM Program API

### Instructions

- `initialize` — 部署桥合约，设置 admin
- `configure_usdc(usdc_mint)` — 设置 USDC mint
- `configure_peer(peer_contract, source_chain_id, target_chain_id)` — 设置对端
- `configure_fee(fee)` — 设置桥手续费（仅 1024chain）
- `configure_rate_limits(max_unlock_per_window, window_duration, max_single_unlock, min_reserve)`
- `stake(amount, receiver_address)` — 锁定 USDC
- `submit_signature(nonce, event_data, signature)` — 提交 Ed25519 签名
- `add_relayer(relayer)` / `remove_relayer(relayer)` / `rotate_relayer(old, new)`
- `propose_admin(new_admin)` / `accept_admin()`
- `pause()` / `unpause()`
- `close_request(nonce)` — 解锁后回收 rent
- `add_liquidity(amount)` / `withdraw_liquidity(amount)`

---

## Relayer API

### Health Endpoint
`GET /health` — 返回 200 表示健康

### Environment Variables
| 变量 | 说明 |
|------|------|
| `BRIDGE_ID` | 桥对标识符（如 `arbsep-1024test-usdc`） |
| `CONFIG_PATH` | 配置文件路径 |
| `QUEUE_DIR` | 事件队列目录 |
| `RELAYER_PRIVATE_KEY` | Relayer 私钥 |
| `ROLE` | `listener`、`submitter` 或 `both` |
| `HEALTH_PORT` | 健康检查端口（默认 8080/8081） |
