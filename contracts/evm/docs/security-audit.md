# Bridge1024 EVM 合约安全审计

## 高危 (High)

### H-1: `configure` 缺少关键参数校验

**位置**: `src/Bridge1024.sol` — `configure()`

```solidity
function configure(
    address usdcAddress,
    bytes32 peerContract,
    uint64 localChainId,
    uint64 peerChainId
) external onlyAdmin {
    if (usdcAddress == address(0)) revert ZeroAddress();
    usdcContract = _usdcContract;
    peerContract = _peerContract;
    localChainId = _localChainId;
    peerChainId = _peerChainId;
    // ...
}
```

**风险**: 函数仅校验了 `usdcAddress != address(0)`，缺少以下校验：

- `peerContract != bytes32(0)` — 如果设为零，`confirmEvent` 的 `sourceContract` 校验形同虚设
- `localChainId != 0` 和 `peerChainId != 0` — 零值链 ID 无实际意义
- `localChainId != peerChainId` — 相同链 ID 可能导致自环回路

**建议**: 添加以上参数校验：

```solidity
if (peerContract == bytes32(0)) revert ZeroAddress();
if (localChainId == 0 || peerChainId == 0) revert InvalidChainId();
if (localChainId == peerChainId) revert InvalidChainId();
```

**状态**: 🟢 已修复 — 添加了 `peerContract != bytes32(0)`、`localChainId/peerChainId != 0`、`localChainId != peerChainId` 校验，新增 `InvalidChainId` 错误

---

### H-3: 无管理操作时间锁

**位置**: `src/Bridge1024.sol` — 所有 `onlyAdmin` 函数

**风险**: Admin 的所有操作（修改配置、添加/移除 relayer、紧急提取、更改速率限制等）都是即时生效的，没有任何 timelock。如果 admin 密钥被攻破（即使是多签），攻击者可立即：

1. 调用 `emergencyWithdraw` 掏空金库
2. 调用 `configureRateLimits(0, 0, 0, 0)` 禁用所有安全限制
3. 添加恶意 relayer 并移除诚实 relayer

**建议**: 对关键操作（特别是 `emergencyWithdraw`、`configure`、`configureRateLimits`）引入 Timelock 机制，在操作生效前提供一段缓冲期，让社区或 guardian 有机会发现并响应异常操作。

**状态**: 🟢 已修复 — 引入 Timelock 机制：`timelockActive` 标志 + `scheduleOperation` / `cancelOperation` / `_consumeTimelock` 流程。初始部署阶段 timelock 未激活，`configure`、`configureRateLimits` 等可即时执行；admin 完成初始配置后调用 `activateTimelock()` 启用，此后 `configure`、`configureRateLimits`、`emergencyWithdraw`、`addRelayer`、`removeRelayer`、`rotateRelayer` 均需先调度（24h 延迟）再执行

---

### H-4: 无最小 relayer 数量约束

**位置**: `src/Bridge1024.sol` — `removeRelayer()`

```solidity
function removeRelayer(address relayerAddress) external onlyAdmin {
    uint256 idx = type(uint256).max;
    for (uint256 i = 0; i < relayers.length; i++) {
        if (relayers[i] == relayerAddress) {
            idx = i;
            break;
        }
    }
    if (idx == type(uint256).max) revert RelayerNotFound();
    relayers[idx] = relayers[relayers.length - 1];
    relayers.pop();
    emit RelayerRemoved(relayerAddress);
}
```

**风险**: Admin 可以移除所有 relayer（导致桥完全冻结），或将 relayer 减至 1 个（阈值 = 1，单点信任）。没有最小数量约束意味着桥的安全性可以被任意降低。

**建议**: 添加最小 relayer 数量常量及检查：

```solidity
uint8 public constant MIN_RELAYERS = 3;

function removeRelayer(address relayerAddress) external onlyAdmin {
    if (relayers.length <= MIN_RELAYERS) revert TooFewRelayers();
    // ...
}
```

**状态**: 🟡 接受风险 — admin 为多签钱包，具备充分治理保障

---

### H-5: `setOperator` 未受时间锁保护 — 绕过 Timelock 的资金窃取路径

**位置**: `src/Bridge1024.sol` — `setOperator()` + `refund()`

```solidity
function setOperator(address _operator) external onlyAdmin {
    address old = operator;
    operator = _operator;
    emit OperatorUpdated(old, _operator);
}
```

**风险**: `activateTimelock()` 后，`emergencyWithdraw`、`configure`、`addRelayer` 等关键函数都受 24 小时时间锁保护，但 `setOperator` 可**即时生效**。这为 admin 密钥泄露场景提供了一条绕过 timelock 的攻击路径：

1. 攻击者立即调用 `setOperator(attacker)` — 无需等待
2. 以新 operator 身份对所有已 stake 的 nonce 调用 `refund(nonce, attacker)` — 窃取全部在途资金
3. 同时调用 `skipNonce` 阻止合法的 unlock

**影响**: 可窃取所有在途（已 stake 未 unlock/refund）的 USDC，绕过了 timelock 本应提供的缓冲窗口。虽然无法通过此路径掏空金库的 reserve 部分（需要时间锁保护的 `emergencyWithdraw`），但在流量高峰期在途资金可能相当可观。

**建议**: 将 `setOperator` 纳入 timelock 保护：

```solidity
function setOperator(address _operator) external onlyAdmin {
    _consumeTimelock(keccak256(abi.encode("setOperator", _operator)));
    address old = operator;
    operator = _operator;
    emit OperatorUpdated(old, _operator);
}
```

**状态**: 🟢 已修复 — `setOperator` 已纳入 `_consumeTimelock` 保护，timelock 激活后需先调度再执行

---

### H-6: `refund` 完全绕过速率限制和储备金检查

**位置**: `src/Bridge1024.sol` — `refund()`

```solidity
function refund(
    uint64 nonce,
    address to
) external onlyOperator nonReentrant {
    if (to == address(0)) revert ZeroAddress();
    uint256 amount = uint256(stakeAmounts[nonce]);
    if (amount == 0) revert ZeroAmount();
    if (refundedNonces[nonce]) revert AlreadyRefunded();
    refundedNonces[nonce] = true;
    IERC20(usdcContract).safeTransfer(to, amount);
    emit Refunded(nonce, to, amount);
}
```

**风险**: `refund` 不调用 `_checkRateLimit`，也不检查 `_checkVaultInvariant`。一个被泄露的 operator（或通过 H-5 路径取得 operator 权限的攻击者）可以在极短时间内批量调用 `refund` 提走所有 staked 金额，完全不受速率限制和最低储备金的约束。

**与 H-5 构成组合攻击**: 泄露的 admin 密钥 → 即时更换 operator → 批量 refund 无限速 → 窃取全部在途资金。

**建议**: 在 `refund` 中加入速率限制和储备金检查：

```solidity
function refund(uint64 nonce, address to) external onlyOperator nonReentrant {
    if (to == address(0)) revert ZeroAddress();
    uint256 amount = uint256(stakeAmounts[nonce]);
    if (amount == 0) revert ZeroAmount();
    if (refundedNonces[nonce]) revert AlreadyRefunded();

    _checkRateLimit(amount);
    _checkVaultInvariant(amount);

    refundedNonces[nonce] = true;
    IERC20(usdcContract).safeTransfer(to, amount);
    emit Refunded(nonce, to, amount);
}
```

**状态**: 🟢 已修复 — `refund` 中添加了 `_checkRateLimit(amount)` 和 `_checkVaultInvariant(amount)` 检查，与 `confirmEvent` 的 unlock 分支保持一致

---

### H-7: `refund` 允许 operator 将退款发送到任意地址 — operator 泄露即可窃取在途资金

**位置**: `src/Bridge1024.sol` — `refund()`

```solidity
function refund(
    uint64 nonce,
    address to  // operator 可指定任意退款地址
) external onlyOperator nonReentrant {
    // ...
    IERC20(usdcContract).safeTransfer(to, amount);
}
```

**风险**: `refund` 的 `to` 参数完全由 operator 控制。如果 operator 密钥泄露（或通过 H-5 路径取得 operator 权限），攻击者可对所有已 stake 的 nonce 调用 `refund(nonce, attacker)` 将资金转走，而非退还给原始 staker。

**与 H-5/H-6 构成组合攻击**: 泄露的 admin → 等待 timelock → 更换 operator → 批量 refund 到攻击者地址。虽然 H-5 已修复（setOperator 受 timelock 保护），H-6 已修复（refund 受速率限制），但 refund 地址可任意指定仍是一个独立的风险点。

**建议**: 在 `stake` 时记录原始 staker 地址，`refund` 直接退回给原始 staker：

```solidity
mapping(uint64 => address) public stakeOwners;

function stake(...) {
    // ...
    stakeOwners[currentNonce] = msg.sender;
}

function refund(uint64 nonce) external onlyOperator nonReentrant {
    address to = stakeOwners[nonce];
    // ...
    IERC20(usdcContract).safeTransfer(to, amount);
}
```

**状态**: 🟢 已修复 — 添加 `stakeOwners` mapping 在 `stake()` 时记录 `msg.sender`，`refund()` 移除 `to` 参数，强制退回原始 staker。即使 operator 被盗也无法将资金转到攻击者地址

---

## 中危 (Medium)

### M-1: USDC 黑名单可导致 nonce 永久阻塞

**位置**: `src/Bridge1024.sol` — `confirmEvent()` 的 unlock 分支

```solidity
confirmation.isProcessed = true;
confirmation.isUnlocked = true;

IERC20(usdcContract).safeTransfer(receiver, unlockAmount);
```

**风险**: 如果 `receiver` 被 USDC 合约列入黑名单（如 OFAC 制裁地址），`safeTransfer` 会永远 revert。由于状态变更和转账在同一事务中，事务回滚后 nonce 保持未处理状态。后续每次 relayer 尝试确认时，当投票达到阈值会再次触发 transfer → revert 的循环，浪费 relayer 的 gas。

**缓解**: 合约提供了 `skipNonce` + `initiateRefund`/`executeRefund` 机制来处理此情况，但需要 operator 介入识别问题并手动处理。可以考虑将 unlock 改为 Pull 模式（先存后取），避免 push 转账被阻塞：

```solidity
// Pull 模式示例
mapping(address => uint256) public claimable;

// unlock 时不直接转账，而是记录可领取金额
claimable[receiver] += unlockAmount;

// 用户自行领取
function claim() external nonReentrant {
    uint256 amount = claimable[msg.sender];
    if (amount == 0) revert ZeroAmount();
    claimable[msg.sender] = 0;
    IERC20(usdcContract).safeTransfer(msg.sender, amount);
}
```

**状态**: 🟡 已缓解 — `skipNonce` + `initiateRefund`/`executeRefund` 提供了手动挽救路径，可接受

---

### M-4: Timelock 无过期机制 — 陈旧操作可无限期延迟执行

**位置**: `src/Bridge1024.sol` — `_consumeTimelock()` + `scheduleOperation()`

```solidity
function _consumeTimelock(bytes32 opHash) internal {
    if (!timelockActive) return;
    uint256 eta = timelockEta[opHash];
    if (eta == 0) revert TimelockNotScheduled();
    if (block.timestamp < eta) revert TimelockNotReady();
    delete timelockEta[opHash];
    emit OperationExecuted(opHash);
}
```

**风险**: 操作一旦调度且延迟期已过，可**无限期**保持可执行状态。场景：

1. Admin 调度 `configureRateLimits(very_high_limits)` 用于临时应急
2. 应急结束后忘记取消该操作
3. 数月后 admin 密钥泄露，攻击者发现这个未过期的操作
4. 攻击者立即执行，禁用速率限制

**建议**: 添加执行窗口期限（如 48 小时 grace period）：

```solidity
uint256 public constant TIMELOCK_GRACE_PERIOD = 48 hours;

function _consumeTimelock(bytes32 opHash) internal {
    if (!timelockActive) return;
    uint256 eta = timelockEta[opHash];
    if (eta == 0) revert TimelockNotScheduled();
    if (block.timestamp < eta) revert TimelockNotReady();
    if (block.timestamp > eta + TIMELOCK_GRACE_PERIOD) revert TimelockExpired();
    delete timelockEta[opHash];
    emit OperationExecuted(opHash);
}
```

**状态**: 🟢 已修复 — 添加 `TIMELOCK_GRACE_PERIOD = 48 hours` 常量和 `TimelockExpired` 错误，`_consumeTimelock` 中增加 `block.timestamp > eta + TIMELOCK_GRACE_PERIOD` 过期检查

---

### M-3: `receiver` 的 bytes32→address 转换缺少高位零校验

**位置**: `src/Bridge1024.sol` — `confirmEvent()` 的 unlock 分支

```solidity
address receiver = address(uint160(uint256(eventData.receiver)));
```

**风险**: `bytes32` 到 `address` 的转换只取低 20 字节，高 12 字节被静默丢弃。如果 relayer 错误地将 SVM 的 32 字节地址作为 EVM receiver 提交，高位非零部分会被截断，资金可能发送到一个不相关的 EVM 地址，造成资金永久丢失。

**建议**: 添加高位零校验，确保 `receiver` 是合法的 EVM 地址格式（左填充零到 32 字节）：

```solidity
if (uint256(eventData.receiver) >> 160 != 0) revert InvalidReceiver();
address receiver = address(uint160(uint256(eventData.receiver)));
```

**状态**: 🟢 已修复 — 在 unlock 分支中添加 `if (uint256(eventData.receiver) >> 160 != 0) revert InvalidReceiver()` 高位零校验

---

## 低危 (Low)

### L-1: `stake` 中 `block.number` 的 uint64 显式转换无溢出保护

**位置**: `src/Bridge1024.sol` — `stake()`

```solidity
emit StakeEvent(
    bytes32(uint256(uint160(address(this)))),
    peerContract,
    localChainId,
    peerChainId,
    uint64(block.number), // 显式 downcast，溢出时静默截断
    stakeAmount,
    bytes32(uint256(uint160(msg.sender))),
    receiver,
    currentNonce
);
```

**风险**: Solidity 0.8.x 的显式类型转换（downcast）会**静默截断**而非 revert。合约其他地方使用了 `SafeCast.toUint64()` 来处理类似转换（如 `actualAmount.toUint64()`），此处风格不一致。虽然 `uint64` 可以容纳极大的区块号（`2^64 ≈ 1.8 × 10^19`，实际不会溢出），但与 SafeCast 的使用风格不一致可能暗示潜在的审查盲区。

**建议**: 使用 `SafeCast` 保持一致性：

```solidity
uint64(block.number)
// 改为
block.number.toUint64()
```

**状态**: 🟢 已修复 — `uint64(block.number)` 改为 `block.number.toUint64()`，与合约其他处的 SafeCast 使用风格一致

---

### L-3: 缺少 `receive()` / `fallback()` 的显式 ETH 拒绝声明

**位置**: `src/Bridge1024.sol` — 合约级别

**风险**: 合约没有显式声明 `receive()` 或 `fallback()` 函数。虽然缺少这些函数意味着合约默认拒绝直接的 ETH 转账（这是正确行为），但 ETH 仍可通过 `selfdestruct`（已弃用但在部分链仍可用）或 coinbase 奖励被强制发送到合约地址。被困在合约中的 ETH 将**永久无法取出**，因为 `emergencyWithdraw` 仅支持 ERC20 代币。

**建议**: 添加显式的 ETH 拒绝声明以明确意图，并考虑添加 ETH 提取函数：

```solidity
receive() external payable {
    revert();
}

function withdrawETH(address payable to) external onlyAdmin {
    if (to == address(0)) revert ZeroAddress();
    uint256 balance = address(this).balance;
    if (balance == 0) revert ZeroAmount();
    (bool success, ) = to.call{value: balance}("");
    require(success);
}
```

**状态**: 🟢 已修复 — 添加了 `receive() external payable { revert(); }` 显式拒绝 ETH 转账，以及 `withdrawETH(address payable to)` 函数提取被 selfdestruct 等强制发送的 ETH

---

### L-4: `withdrawETH` 缺少 `nonReentrant` 修饰器

**位置**: `src/Bridge1024.sol` — `withdrawETH()`

```solidity
function withdrawETH(address payable to) external onlyAdmin {
    if (to == address(0)) revert ZeroAddress();
    uint256 balance = address(this).balance;
    if (balance == 0) revert ZeroAmount();
    (bool success, ) = to.call{value: balance}("");
    if (!success) revert ETHTransferFailed();
    emit ETHWithdrawn(to, balance);
}
```

**风险**: 虽然自然的 checks-effects 模式（重入时余额为 0 → revert `ZeroAmount`）使得重入攻击无效，且 `to` 由 admin 指定，但与合约中 `emergencyWithdraw`、`refund`、`confirmEvent` 均使用 `nonReentrant` 的风格不一致。在安全审计中，不一致性往往是审查盲区。

**建议**: 添加 `nonReentrant` 保持一致性：

```solidity
function withdrawETH(address payable to) external onlyAdmin nonReentrant {
```

**状态**: 🟢 已修复 — `withdrawETH` 已添加 `nonReentrant` 修饰器，与 `emergencyWithdraw`、`refund`、`confirmEvent` 风格一致

---

### L-5: `setGuardian` 未受时间锁保护

**位置**: `src/Bridge1024.sol` — `setGuardian()`

```solidity
function setGuardian(address _guardian) external onlyAdmin {
    address old = guardian;
    guardian = _guardian;
    emit GuardianUpdated(old, _guardian);
}
```

**风险**: 泄露的 admin 密钥可立即替换 guardian。虽然 guardian 只能 pause（不能 unpause），但这削弱了 "guardian 作为独立安全角色" 的设计意图。攻击者可以立即将 guardian 设为自己控制的地址，消除原 guardian 的紧急暂停能力（如果原 guardian 正尝试暂停来阻止攻击）。

**建议**: 将 `setGuardian` 也纳入 timelock 保护：

```solidity
function setGuardian(address _guardian) external onlyAdmin {
    _consumeTimelock(keccak256(abi.encode("setGuardian", _guardian)));
    address old = guardian;
    guardian = _guardian;
    emit GuardianUpdated(old, _guardian);
}
```

**状态**: 🟢 已修复 — `setGuardian` 已纳入 `_consumeTimelock` 保护，timelock 激活后需先调度再执行

---

### L-6: `configureRateLimits` 不校验参数合理性

**位置**: `src/Bridge1024.sol` — `configureRateLimits()`

```solidity
function configureRateLimits(
    uint256 _maxPerWindow,
    uint256 _windowDuration,
    uint256 _maxSingle,
    uint256 _minReserve
) external onlyAdmin {
    _consumeTimelock(...);
    maxUnlockPerWindow = _maxPerWindow;
    windowDuration = _windowDuration;
    maxSingleUnlock = _maxSingle;
    minimumReserve = _minReserve;
    // ...
}
```

**风险**: 允许设置矛盾的参数组合：

- `maxSingleUnlock > maxUnlockPerWindow`（两者都非零时）— 单笔限额大于窗口限额，单笔限制形同虚设
- `windowDuration = 1` — 速率限制每秒重置，等同于无限制
- `minimumReserve` 大于当前金库余额 — 所有 unlock 立即被阻塞

**建议**: 添加基本的参数关系校验：

```solidity
if (_maxPerWindow != 0 && _maxSingle != 0 && _maxSingle > _maxPerWindow)
    revert InvalidRateLimitParams();
if (_maxPerWindow != 0 && _windowDuration != 0 && _windowDuration < 60)
    revert InvalidRateLimitParams();
```

**状态**: 🟢 已修复 — 添加 `InvalidRateLimitParams` 错误，校验 `maxSingle <= maxPerWindow`（双非零时）和 `windowDuration >= 60`（启用时）

---

### L-7: `stake` 无金额上限 — 用户 stake 后可能在对端无法 unlock

**位置**: `src/Bridge1024.sol` — `stake()`

**风险**: `stake` 不校验金额上限。如果对端链配置了 `maxSingleUnlock`（如 500 USDC），用户在本链 stake 1000 USDC 后，对端 `confirmEvent` 达到阈值时 unlock 会被 `SingleTransferExceeded` 拦截，nonce 卡死。此时必须由 operator 手动介入：先在对端 `skipNonce`，再在本链 `initiateRefund`/`executeRefund`，增加运维负担且影响用户体验。

**建议**: 添加 `maxStakeAmount` 状态变量，在 `stake()` 中校验实际转入金额。admin 应将其配置为对端链的 `maxSingleUnlock` 值：

```solidity
uint256 public maxStakeAmount;
error StakeAmountExceeded();

function stake(...) {
    // ...
    uint256 actualAmount = usdc.balanceOf(address(this)) - balanceBefore;
    if (maxStakeAmount != 0 && actualAmount > maxStakeAmount)
        revert StakeAmountExceeded();
    // ...
}
```

**状态**: 🟢 已修复 — 添加 `maxStakeAmount` 状态变量和 `StakeAmountExceeded` 错误，`configureRateLimits` 增加 `_maxStake` 参数，`stake()` 中在 fee-on-transfer 余额差计算后校验 `actualAmount <= maxStakeAmount`

---

### L-8: `configureRateLimits` 不重置窗口状态 — 参数变更后行为不可预测

**位置**: `src/Bridge1024.sol` — `configureRateLimits()`

**风险**: 更改速率限制参数时，`currentWindowStart`、`currentWindowUsage`、`previousWindowUsage` 不会重置。如果将 `maxUnlockPerWindow` 从 1000 降到 300，而当前窗口已用 400，则后续所有 unlock 都会被 `RateLimitExceeded` 阻塞直到窗口自然过期。

**建议**: 重置窗口状态：

```solidity
currentWindowStart = block.timestamp;
currentWindowUsage = 0;
previousWindowUsage = 0;
```

**状态**: 🟢 已修复 — `configureRateLimits` 中在赋值新参数后重置 `currentWindowStart`、`currentWindowUsage`、`previousWindowUsage`

---

### L-9: `refund` 未检查 `maxSingleUnlock` — 单笔限额绕过

**位置**: `src/Bridge1024.sol` — `refund()`

**风险**: `confirmEvent` 的 unlock 分支检查了三层保护（`_checkRateLimit` + `maxSingleUnlock` + `_checkVaultInvariant`），但 `refund` 仅检查两层，缺少 `maxSingleUnlock`。operator 通过 `refund` 可以绕过单笔限额。

**建议**: 在 `refund` 中添加 `maxSingleUnlock` 检查：

```solidity
_checkRateLimit(amount);
if (maxSingleUnlock != 0 && amount > maxSingleUnlock)
    revert SingleTransferExceeded();
_checkVaultInvariant(amount);
```

**状态**: 🟢 已修复 — `refund` 中添加了 `_checkRateLimit(amount)` + `maxSingleUnlock` + `_checkVaultInvariant(amount)` 三层检查，与 `confirmEvent` 的 unlock 分支保持一致。此外 `refund` 已重构为从 `stakeOwners` 读取退款地址（见 H-7），进一步收窄 operator 的权限边界

---

## 已修复 (Resolved)

### ~~H-2: `emergencyWithdraw` 缺少 `nonReentrant` 修饰器~~

**位置**: `src/Bridge1024.sol` — `emergencyWithdraw()`

**风险**: `emergencyWithdraw` 调用了外部合约的 `safeTransfer`，如果代币合约存在恶意 hook，可能触发重入攻击。

**修复**: 已添加 `nonReentrant` 修饰器：

```solidity
function emergencyWithdraw(
    address token, uint256 amount, address to
) external onlyAdmin nonReentrant { ... }
```

---

### ~~M-2: `proposeAdmin` 无法取消待处理的管理员转移~~

**位置**: `src/Bridge1024.sol` — `proposeAdmin()`

**风险**: 一旦提议新管理员，无法撤回。如果提议了错误地址，只能等待或再次提议覆盖。

**修复**: 移除了 `address(0)` 的零地址校验，允许传入 `address(0)` 取消当前提议：

```solidity
function proposeAdmin(address newAdmin) external onlyAdmin {
    pendingAdmin = newAdmin; // address(0) = 取消提议
    emit AdminTransferProposed(admin, newAdmin);
}
```

---

### ~~L-2: `receiverAddress` 使用 string 类型导致 gas 浪费和解析复杂度~~

**位置**: `src/Bridge1024.sol` — `StakeEventData.receiverAddress`、`stake()`、`confirmEvent()`

**风险**: `string` 是动态类型，`abi.encode` 和 `keccak256` 操作比定长类型更贵。合约需要维护 `_parseAddress`（hex 字符串解析为 address）和 `_validateReceiverAddress`（字符合法性校验）两个内部函数，增加了攻击面和代码复杂度。

**修复**: `receiverAddress` 从 `string` 改为 `bytes32 receiver`。EVM 地址右对齐 20 字节存入 bytes32，SVM Pubkey 原生 32 字节直接存入。删除了 `_parseAddress`、`_validateReceiverAddress` 两个内部函数和 `InvalidReceiverAddress` 错误。`stake()` 参数改为 `bytes32 receiver`，仅需校验 `!= bytes32(0)`。`confirmEvent()` 中用 `address(uint160(uint256(eventData.receiver)))` 直接提取地址。

**效果**:
- 节省 gas：`abi.encode` 全定长结构，hash 更高效
- 减少攻击面：无字符串解析逻辑
- 跨链统一：与 SVM 的 `Pubkey`（32 字节）格式对齐

---

## 二轮审计新增 (Round 2)

### NEW-M1: 被移除的 relayer 已提交的确认投票仍然有效

**位置**: `src/Bridge1024.sol` — `confirmEvent()` 投票机制 + `removeRelayer()`

**风险**: 当一个 relayer 被从白名单移除时，其已经提交的确认投票（存储在 `NonceConfirmation.confirmedRelayers` 和 `hashVotes` 中）不会被清除，仍然计入阈值。场景：3 个 relayers (A, B, C)，`frozenThreshold = 2`。Relayer A 对 nonce X 提交确认后被移除（发现已被入侵），Relayer B 随后提交确认 → 投票数达到 2 → 触发 unlock。此时 A 的旧投票仍生效。

**缓解因素**:
- 如果被入侵的 relayer 提交的是错误数据（不同 hash），则不影响正确数据的投票
- Operator 可在移除 relayer 后对相关 in-flight nonce 调用 `skipNonce` 处理
- 实现 epoch 机制使旧投票失效会显著增加复杂度和 gas 成本

**状态**: 🟡 接受风险 — 通过运维流程（移除 relayer 时同步检查 in-flight nonce）缓解

---

### NEW-M2: `executeRecovery` 不清除已调度的 timelock 操作

**位置**: `src/Bridge1024.sol` — `executeRecovery()`

**风险分析**: 紧急恢复时 `timelockEta` 中已调度的操作未被清除。但此问题**实际不构成威胁**：`executeRecovery` 将 `admin` 替换为新地址后，旧 admin 不再通过 `onlyAdmin` 检查，即使 timelock 到期也无法调用任何受保护函数。已调度操作会在 `TIMELOCK_GRACE_PERIOD`（48h）后自动过期。

**结论**: ❌ 已否定 — 旧 admin 在 recovery 后丧失一切权限，无法执行已调度操作。新 admin 如需谨慎，可主动调用 `cancelOperation` 清除遗留调度。

---

### NEW-L1: `configureRateLimits` 允许 `maxPerWindow` 和 `windowDuration` 不一致为零

**位置**: `src/Bridge1024.sol` — `configureRateLimits()`

**风险**: `maxPerWindow > 0` 而 `windowDuration = 0` 时，`_checkRateLimit` 因 `windowDuration == 0` 直接返回，速率限制被静默禁用，与 `maxPerWindow` 的非零值暗示的意图矛盾。反之亦然。

**建议**: 要求两者同时为零（禁用）或同时非零（启用）：

```solidity
if ((_maxPerWindow == 0) != (_windowDuration == 0))
    revert InvalidRateLimitParams();
```

**状态**: 🟢 已修复 — 添加校验确保 `maxPerWindow` 和 `windowDuration` 同时为零或同时非零

---

### NEW-L2: `setRecoveryAddress` / `recoveryAddress` 命名与 `setGuardian` / `setOperator` 风格不一致

**位置**: `src/Bridge1024.sol` — 状态变量、函数、事件命名

**风险**: `guardian`、`operator` 的状态变量和 setter 函数分别命名为 `setGuardian`、`setOperator`，但 recovery 角色使用了 `recoveryAddress` 和 `setRecoveryAddress`，事件为 `RecoveryAddressUpdated`。命名风格不一致增加代码维护和审查的认知负担。

**建议**: 统一命名风格：
- `recoveryAddress` → `recovery`
- `setRecoveryAddress` → `setRecovery`
- `RecoveryAddressUpdated` → `RecoveryUpdated`

**状态**: 🟢 已修复 — 状态变量、函数、事件、timelock 编码字符串均已重命名

---

## 三轮审计新增 (Round 3)

### R3-M1: `executeRecovery` 后遗留的 Timelock 可被新 admin 意外消费

**位置**: `src/Bridge1024.sol` — `executeRecovery()` + `_consumeTimelock()`

**风险**: 二轮审计的 NEW-M2 分析认为"旧 admin 在 recovery 后丧失权限，无法执行已调度操作"而否定了此问题。但该分析忽略了一种场景：**新 admin 可能意外消费旧 admin 遗留的调度**。

攻击路径：

1. 被攻破的 admin 调度恶意操作，例如 `scheduleOperation(abi.encode("addRelayer", maliciousAddress))`
2. Guardian 检测到并冻结合约
3. Recovery 设置新 admin 并解冻（`timelockEta` 中遗留的调度未被清除）
4. 攻击者通过社工手段引导新 admin 执行相同参数的操作
5. 新 admin 调用 `addRelayer(maliciousAddress)` → `_consumeTimelock` 发现旧调度的 ETA 已过且在 grace period 内 → 直接执行，跳过 24h 等待

**缓解因素**:
- 攻击者需要精确猜测或影响新 admin 的操作参数
- Grace period 为 72h（24h delay + 48h window），超时后自动失效
- `cancelOperation` 已移除 `whenNotPaused` 限制（R3-L4），新 admin 可在任何状态下清除遗留调度

**状态**: 🟡 接受风险 — 新 admin 应在执行任何操作前通过 `OperationScheduled` 事件索引遗留调度，并调用 `cancelOperation` 清除

---

### R3-M2: Guardian 恶意冻结可形成持续 DoS 循环

**位置**: `src/Bridge1024.sol` — `emergencyFreeze()` + `executeRecovery()` + `setGuardian()`

**风险**: 如果 guardian 密钥被盗，攻击者可利用以下循环对合约进行持续 DoS：

1. Guardian 冻结合约
2. Recovery 解冻并设新 admin
3. 新 admin 调度 `setGuardian(newGuardian)`（需 24h timelock）
4. Guardian 立即再次冻结（< 1 秒后）
5. 回到步骤 2，循环持续

由于 `setGuardian` 受 24h Timelock 保护，而 `emergencyFreeze` 是即时操作，恶意 guardian 总能在新 guardian 设置生效前抢先冻结。

**建议**: 为 `executeRecovery` 增加可选的 guardian 替换参数，使 recovery 可在一笔交易中同时替换 admin 和 guardian，打破 DoS 循环。

**状态**: 🟢 已修复 — `executeRecovery` 新增 `newGuardian` 参数，传 `address(0)` 保留当前 guardian，传非零地址则同时替换 guardian

---

### R3-M3: `executeRefund` 与 `confirmEvent` 共享速率限制窗口

**位置**: `src/Bridge1024.sol` — `executeRefund()` 和 `confirmEvent()` 均调用 `_checkTransferLimits()`

**风险**: `executeRefund`（发送端退款）和 `confirmEvent` 的 unlock 分支（接收端解锁）共享同一组速率限制计数器（`currentWindowUsage`、`previousWindowUsage`）。在同一链同时扮演发送方和接收方的场景下，大量退款会消耗 unlock 额度，反之亦然。

场景：窗口限额 1000 USDC，operator 在窗口内退款 800 USDC，则同一窗口内只剩 200 USDC 的 unlock 额度。

**缓解因素**: executeRefund 需 operator 先 initiateRefund 并等待 6h，operator 应协调操作节奏；在实际部署中，发送端和接收端通常是不同链上的不同合约实例。

**状态**: 🟡 接受风险 — 运维文档中明确提醒 executeRefund 和 unlock 共享额度，operator 应在低峰期发起批量退款

---

### R3-L1: 构造函数不强制角色地址分离

**位置**: `src/Bridge1024.sol` — `constructor()`

**风险**: 角色地址可以设为相同值（例如 `admin == guardian`），完全瓦解角色分离安全模型。

**状态**: 🟢 已修复 — 构造函数添加所有角色地址（含 msg.sender 即 admin）两两不等校验，新增 `RoleOverlap` 错误

---

### R3-L2: `confirmEvent` 的 `eventData` 参数应使用 `calldata`

**位置**: `src/Bridge1024.sol` — `confirmEvent()`

**风险**: `memory` 参数需要 EVM 将 calldata 复制到内存，额外消耗 gas。`confirmEvent` 是 relayer 高频调用的函数，gas 优化有实际意义。

**状态**: 🟢 已修复 — `StakeEventData memory eventData` 改为 `StakeEventData calldata eventData`

---

### R3-L3: `withdrawETH` 的 Timelock 哈希不包含金额

**位置**: `src/Bridge1024.sol` — `withdrawETH()`

**风险**: Timelock 仅对目标地址 `to` 做承诺，实际提取金额取决于执行时的 `address(this).balance`。调度时金额未确定，社区/guardian 审查时无法评估转移金额。

**缓解因素**: 合约 `receive()` 拒绝直接 ETH 转账，只有 `selfdestruct` 能强制注入；实际场景中合约的 ETH 余额通常很小。

**状态**: 🟡 接受风险 — ETH 为意外注入资金，金额不可预测，按全额提取符合使用场景

---

### R3-L4: `cancelOperation` 在暂停状态下不可调用

**位置**: `src/Bridge1024.sol` — `cancelOperation()`

**风险**: `cancelOperation` 带有 `whenNotPaused` 修饰器。在紧急冻结期间，admin 无法取消已调度的恶意操作。虽然 `executeRecovery` 会解冻，但解冻后到取消操作之间存在微小时间窗口，恶意 guardian 可能在此窗口内重新冻结。

**建议**: 移除 `cancelOperation` 的 `whenNotPaused` 限制，允许 admin 在任何状态下取消操作。

**状态**: 🟢 已修复 — `cancelOperation` 移除 `whenNotPaused` 修饰器

---

## 四轮审计新增 (Round 4)

### R4-M1: 角色变更函数不强制角色分离 — 可在运行时瓦解安全模型

**位置**: `src/Bridge1024.sol` — `setGuardian()`, `setOperator()`, `setRecovery()`, `acceptAdmin()`, `executeRecovery()`

**风险**: 构造函数通过 `RoleOverlap` 校验强制了四个角色地址两两不等，但所有后续角色变更函数均不检查新地址是否与其他角色重叠。攻击路径：

1. Admin 调度 `setRecovery(adminAddress)` 并执行 — recovery 与 admin 合一
2. Admin 密钥泄露 → 攻击者同时控制 admin 和 recovery
3. 攻击者调度 `setGuardian(attacker2)` 并执行
4. 攻击者控制 admin + guardian + recovery，完全绕过角色分离安全模型

**建议**: 在所有角色变更函数中添加交叉校验，维持构造函数建立的角色分离不变量。

**状态**: 🟢 已修复 — `setGuardian`、`setOperator`、`setRecovery`、`acceptAdmin`、`executeRecovery` 均添加了角色交叉校验，复用 `RoleOverlap` 错误

---

### R4-L1: `scheduleOperation` 使用显式 uint64 转换而非 SafeCast

**位置**: `src/Bridge1024.sol` — `scheduleOperation()`

**风险**: `uint64(block.timestamp)` 使用显式 downcast（静默截断），而合约其他位置统一使用 `SafeCast.toUint64()`。L-1 审计已修复了 `block.number` 的同类问题，此处的 `block.timestamp` 转换被遗漏。

**状态**: 🟢 已修复 — `uint64(block.timestamp)` 改为 `block.timestamp.toUint64()`

---

### R4-L2: `proposeAdmin` 执行后无法直接撤回 pendingAdmin

**位置**: `src/Bridge1024.sol` — `proposeAdmin()`

**风险**: `proposeAdmin(addr)` 通过 Timelock 执行后，`pendingAdmin` 被设为 `addr`，且 `address(0)` 会 revert，无法直接清除。要撤回需再走一轮 24h Timelock。

**状态**: 🟡 接受风险 — `proposeAdmin` 本身受 24h Timelock 保护，问题在调度窗口内通过 `cancelOperation` 解决即可；`pendingAdmin` 只能由自身 `acceptAdmin()`，不构成安全威胁

---

### R4-L3: `withdrawToken` 可绕过 `minimumReserve` 保护

**位置**: `src/Bridge1024.sol` — `withdrawToken()`

**风险**: `withdrawToken` 不调用 `_checkVaultInvariant`，admin 可提取 USDC 至余额低于 `minimumReserve`。

**状态**: 🟡 接受风险 — `withdrawToken` 是流动性管理/桥退役用途，受 24h Timelock 保护，admin（多签）全权负责

---

### R4-I1: `isRelayer` 使用 O(n) 数组遍历

**位置**: `src/Bridge1024.sol` — `isRelayer()`

**风险**: `confirmEvent` 每次调用时遍历 relayers 数组（O(n)），可改为 `mapping(address => bool)` 实现 O(1)。

**状态**: 🟡 接受风险 — n ≤ 18（MAX_RELAYERS），gas 差异约 ~2000 gas，可忽略

---

### R4-I2: Solidity 版本使用浮动约束

**位置**: `src/Bridge1024.sol` — `pragma solidity ^0.8.20`

**风险**: `^0.8.20` 允许任意 0.8.x 版本 >= 0.8.20，不同 patch 版本可能引入微妙差异。

**状态**: 🟢 已修复 — 锁定为 `pragma solidity 0.8.20`

---

## 第 5 轮审计补丁 (R5)

本轮聚焦"角色治理边界"与"转出限额职责切分"两类问题，全部已落地，对应单元测试已并入 `test/Bridge1024.t.sol`，Foundry 套件 131 / 131 通过（含 256 次 fuzz）。

---

### R5-INV1: refund 路径不应受 `maxSingleUnlock` 约束 — "能 stake 必须能 refund" 不变量

**位置**: `src/Bridge1024.sol` — `_checkTransferLimits()` / `executeRefund()`

**风险**: 原 `_checkTransferLimits` 同时被 `confirmEvent`（unlock 入金）与 `executeRefund`（退款出金）调用，并在内部统一执行 `if (amount > maxSingleUnlock) revert SingleTransferExceeded()`。这导致一个明显的不变量违反：

```solidity
// 用户 stake：通过了 maxStakeAmount 校验
bridge.stake(nonce, 1000e6, receiver);

// admin 事后把 maxSingleUnlock 调小到 500e6
bridge.configureRateLimits(_, _, 500e6, _, _);

// 退款被卡住 —— 用户资金永久无法取回
bridge.executeRefund(nonce); // revert SingleTransferExceeded
```

`maxSingleUnlock` 在语义上是"对端来的解锁/到账"侧的防御（防止可疑的大额跨链入金），不应反向作用于用户自己的退款。同时 `maxStakeAmount` 已经在 stake 阶段做过单笔上限把关，refund 再做一次 `maxSingleUnlock` 校验属于双重约束、且方向错误。

**修复**:

1. `confirmEvent` 入口前置一次 `maxSingleUnlock` 早拒（见 R5-L2），避免 relayer 浪费 gas 投到阈值才被回滚；
2. `_checkTransferLimits` 内部去掉 `maxSingleUnlock` 检查，仅保留**滑动窗口速率限制**与**金库储备不变量**（两条对 unlock / refund 都有意义）；
3. 新增不变量测试：
   - `testRefund_NotBlockedByMaxSingleUnlock`：stake 1000e6 → admin 收紧 `maxSingleUnlock` 到 500e6 → refund 仍必须成功；
   - `testConfirmEvent_StillBoundByMaxSingleUnlock`：unlock 路径仍受 `maxSingleUnlock` 约束（镜像测试，确保不放行）。

**状态**: 🟢 已修复 — `_checkTransferLimits` 改为 `_checkRateLimit + _checkVaultInvariant`，`maxSingleUnlock` 唯一来源迁至 `confirmEvent` 入口

---

### R5-M1: `proposeAdmin` 与 `setGuardian/Operator/Recovery` 缺少角色重叠预检

**位置**: `src/Bridge1024.sol` — `proposeAdmin()` / `setGuardian()` / `setOperator()` / `setRecovery()`

**风险**:

1. `proposeAdmin(addr)` 只在 `acceptAdmin` 阶段才校验 `addr ∉ {guardian, operator, recovery}`。如果 admin 不慎把 `pendingAdmin` 提议为现有角色地址，**24h Timelock 调度会被白白消耗**，且 `pendingAdmin` 会卡在该地址，因为：
   - `acceptAdmin` 永远 revert（`RoleOverlap`）；
   - 撤回又得再走一轮 24h Timelock。

2. `setGuardian / setOperator / setRecovery` 只校验新角色 ∉ `{admin, 其它两个角色}`，**未校验 `!= pendingAdmin`**。如果在 `pendingAdmin` 已设置的窗口内把某个角色改成 `pendingAdmin`，同样会让 `acceptAdmin` 永久卡死。

**修复**:

1. `proposeAdmin` 增加预检：`newAdmin ∉ {admin, guardian, operator, recovery}`，提前 revert 避免 Timelock 白费；
2. `setGuardian / setOperator / setRecovery` 在 `RoleOverlap` 检查中追加 `!= pendingAdmin` 一项；
3. `executeRecovery` 已在末尾把 `pendingAdmin` 重置为 `address(0)`，无需额外修改；
4. 新增测试：
   - `testProposeAdmin_RoleOverlap`：覆盖 4 种角色重叠的预检；
   - `testSetGuardian_RoleOverlap_PendingAdmin` / `testSetOperator_RoleOverlap_PendingAdmin` / `testSetRecovery_RoleOverlap_PendingAdmin`：覆盖与 `pendingAdmin` 的重叠；
   - `testAcceptAdmin_RoleOverlap` 改为只验证合法路径（深度防御分支已不可达）。

**状态**: 🟢 已修复

---

### R5-L1: `fallback()` 未定义 — 未知 calldata 行为不显式

**位置**: `src/Bridge1024.sol` — 合约根入口

**风险**: 已有 `receive() external payable { revert(); }` 显式拒绝纯 ETH 转账，但**未定义 `fallback()`**。携带未知 selector 的 calldata（无论是否带 ETH）会因找不到匹配函数而隐式 revert，行为依赖 EVM 默认而非显式声明，意图不清晰且无法被静态分析工具明确捕捉。

**修复**: 新增显式 `fallback() external payable { revert(); }`，与 `receive()` 配合明确拒绝所有非函数调用入口。新增测试 `testFallback_RevertsOnUnknownCalldata` 覆盖：未知 selector、未知 selector + 携带 ETH、纯 ETH 转账三种情形。

**状态**: 🟢 已修复

---

### R5-L2: `confirmEvent` 阶段未提前校验 `maxSingleUnlock` — 浪费 relayer gas

**位置**: `src/Bridge1024.sol` — `confirmEvent()`

**风险**: 原实现里 `maxSingleUnlock` 检查发生在阈值满足、即将真正 unlock 的时刻。这意味着：当某条 `eventData.amount > maxSingleUnlock` 的非法事件被广播时，**所有 relayer 都会成功投票**，直到最后一个达成阈值的 relayer 触发 `_checkTransferLimits` 才被 revert。前面投票的 relayer 已经付出 gas，且 `NonceConfirmation` 状态被污染。

**修复**: 在 `confirmEvent` 入口前段（在初始化 `frozenThreshold` / `confirmedRelayers` 之前）加一次早拒：

```solidity
if (maxSingleUnlock != 0 && eventData.amount > maxSingleUnlock)
    revert SingleTransferExceeded();
```

第一个 relayer 提交即被拒，nonce 状态零残留。同时由 R5-INV1 一并迁移走 `_checkTransferLimits` 内的 `maxSingleUnlock`，使得整个合约里 `maxSingleUnlock` 的语义只在这一个地方表达。新增测试 `testConfirmEvent_EarlyMaxSingleReject` / `testConfirmEvent_EarlyMaxSingleSkippedWhenZero`，并把原 `testRateLimit_MaxSingle` 调整为 "第一个 relayer 即被拒" 的语义。

**状态**: 🟢 已修复

---

### R5-H2: `_checkRateLimit` 累加路径在极端配置下的显式 SafeCast 防御

**位置**: `src/Bridge1024.sol` — `_checkRateLimit()`

**背景**: 原代码 `currentWindowUsage += amount` 依赖 Solidity 0.8 的内置溢出 revert。理论上：

```
slidingUsage + amount <= maxUnlockPerWindow      // 上一行的检查保证
currentWindowUsage <= slidingUsage               // 滑动窗口语义保证
=> currentWindowUsage + amount <= maxUnlockPerWindow <= type(uint64).max
```

因此累加在算法上**永远不可能溢出**。但这是一个隐式约束，依赖读者推导滑动窗口的不变量；如果未来某次重构（改公式、改 `_maxPerWindow` 类型、改 `slidingUsage` 计算顺序）破坏前置条件，溢出风险会以"看起来正常的代码"形式出现。

**修复**: 把累加改写为显式 SafeCast，让"不溢出"这条不变量在代码中显形：

```solidity
currentWindowUsage = (uint256(currentWindowUsage) + amount).toUint64();
```

行为等价（同样在溢出时 revert），但意图固化下来：任意 future-edit 破坏不变量都会被 SafeCast 在边界明确捕捉，而不是依赖编译器的隐式行为。新增 `testRateLimit_U64Boundary_NoSilentTruncation` 与 fuzz `testFuzz_RateLimit_MonotonicAndBounded`（256 runs），覆盖极端 `maxPerWindow = type(uint64).max` 与任意合法配置下的累加单调性。

**状态**: 🟢 已修复（防御性，无功能影响）
