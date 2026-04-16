# Bridge1024 EVM 合约运维手册

## 目录

- [1. 架构概览](#1-架构概览)
- [2. 部署流程](#2-部署流程)
- [3. Timelock 操作规范](#3-timelock-操作规范)
- [4. 日常运维](#4-日常运维)
- [5. 紧急响应](#5-紧急响应)
- [6. Nonce 异常处理（skipNonce + 两步退款）](#6-nonce-异常处理skipnonce--两步退款)
- [7. 配置变更](#7-配置变更)
- [8. 管理员转移](#8-管理员转移)
- [9. 流动性管理](#9-流动性管理)
- [10. 桥退役](#10-桥退役)
- [11. 常见错误排查](#11-常见错误排查)
- [附录 A：完整函数权限表](#附录-a完整函数权限表)
- [附录 B：事件监控清单](#附录-b事件监控清单)

---

## 1. 架构概览

### 1.1 角色体系

合约采用四角色分离设计，所有角色地址必须两两不同：

| 角色 | 推荐载体 | 职责 | 泄露最坏影响 |
|------|----------|------|--------------|
| **admin** | 多签钱包 (如 Safe) | 全权管理：配置、relayer 管理、资金提取、角色变更 | 受 24h Timelock 保护，guardian 可在窗口内冻结 |
| **guardian** | EOA（热钱包） | 唯一能力：`emergencyFreeze()` 紧急冻结 | DoS（需 recovery 解冻），不丢资金 |
| **operator** | EOA（热钱包） | `skipNonce` + `initiateRefund`，日常运维 | 只能发起退款（6h 后执行），不能改退款地址，admin 可取消 |
| **recovery** | 冷钱包 / 硬件多签 | 仅在冻结后可用：更换 admin + 可选更换 guardian + 解冻 | 单独持有无法操作（需 guardian 先冻结） |

### 1.2 跨链流程

```
源链 (发送方)                                      目标链 (接收方)
─────────────                                    ─────────────
用户调用 stake()                                  
  → USDC 锁入合约                                
  → 发出 StakeEvent                               中继者监听 StakeEvent
                                                   → 调用 confirmEvent()
                                                   → 投票计数
                                                   → 达到 2/3 阈值
                                                   → 自动 unlock USDC 到 receiver
```

同一合约实例同时扮演发送方和接收方角色。

### 1.3 安全分层

```
第 1 层：权限控制      — 四角色分离 + Timelock
第 2 层：投票阈值      — 2/3 多数 relayer 确认
第 3 层：速率限制      — 滑动窗口 + 单笔限额
第 4 层：储备金保护    — minimumReserve 不变量
第 5 层：紧急响应      — guardian 冻结 + recovery 恢复
```

---

## 2. 部署流程

### 2.1 前置准备

- [ ] 准备 admin 多签钱包地址（推荐 Safe，3/5 或更高阈值）
- [ ] 准备 guardian EOA 地址（响应速度优先）
- [ ] 准备 operator EOA 地址（日常运维用）
- [ ] 准备 recovery 冷钱包地址（离线保管）
- [ ] 确认目标链 USDC 合约地址
- [ ] 确认对端桥合约地址（如果同时部署两端，需要先部署一端获取地址）
- [ ] 确定本链和对端链的 Chain ID
- [ ] 准备至少 3 个 relayer 地址
- [ ] 准备初始流动性 USDC

### 2.2 部署合约

```solidity
// 部署者（msg.sender）自动成为初始 admin
Bridge1024 bridge = new Bridge1024(
    0xGuardian...,  // _guardian: EOA 热钱包
    0xOperator...,  // _operator: EOA 热钱包
    0xRecovery...   // _recovery: 冷钱包
);
```

部署者自动成为初始 admin，后续可通过 `proposeAdmin` + `acceptAdmin` 转移给多签钱包。
部署后合约处于 **Timelock 未激活** 状态，admin 可以直接调用所有管理函数无需等待。

### 2.3 初始配置（Timelock 未激活，可即时执行）

按以下顺序完成配置：

**第 1 步：配置核心参数**

```solidity
bridge.configure(
    0xUSDC...,                              // USDC 合约地址
    bytes32(uint256(uint160(0xPeer...))),    // 对端桥合约地址（bytes32 格式）
    1,                                       // 本链 Chain ID
    2                                        // 对端链 Chain ID
);
```

> 对端合约地址转换：EVM 地址右对齐填充到 bytes32，SVM 地址直接使用 32 字节 Pubkey。

**第 2 步：配置速率限制**

参考 `rate-limits-guide.md` 选择方案。初期推荐保守型：

```solidity
bridge.configureRateLimits(
    10_000e6,   // 每小时最多解锁 10,000 USDC
    3600,       // 窗口 = 1 小时
    5_000e6,    // 单笔最大 5,000 USDC
    5_000e6,    // 单笔最大 stake 5,000 USDC（匹配对端 maxSingle）
    20_000e6    // 金库至少保留 20,000 USDC
);
```

**第 3 步：添加中继者**

至少添加 3 个，确保 2/3 容错：

```solidity
bridge.addRelayer(0xRelayer1...);
bridge.addRelayer(0xRelayer2...);
bridge.addRelayer(0xRelayer3...);
```

| relayer 数量 | 阈值 (ceil(n*2/3)) | 容错数 |
|-------------|-------------------|--------|
| 3 | 2 | 1 |
| 5 | 4 | 1 |
| 7 | 5 | 2 |
| 9 | 6 | 3 |
| 12 | 8 | 4 |
| 18 | 12 | 6 |

**第 4 步：注入初始流动性**

直接向桥合约地址转入 USDC（通过 ERC20 `transfer`）：

```solidity
usdc.transfer(address(bridge), 100_000e6);  // 注入 100,000 USDC
```

**第 5 步：验证配置**

```solidity
// 检查核心参数
assert(bridge.admin() == 0xAdmin...);
assert(bridge.usdcContract() == 0xUSDC...);
assert(bridge.localChainId() == 1);
assert(bridge.peerChainId() == 2);

// 检查中继者
assert(bridge.getRelayerCount() == 3);
assert(bridge.isRelayer(0xRelayer1...));

// 检查速率限制
assert(bridge.maxUnlockPerWindow() == 10_000e6);
assert(bridge.windowDuration() == 3600);
assert(bridge.maxSingleUnlock() == 5_000e6);

// 检查角色
assert(bridge.guardian() == 0xGuardian...);
assert(bridge.operator() == 0xOperator...);
assert(bridge.recovery() == 0xRecovery...);
```

**第 6 步：激活 Timelock**

⚠️ **此操作不可逆。** 激活后所有关键管理操作需要 24h 延迟。

```solidity
bridge.activateTimelock();
```

确认：

```solidity
assert(bridge.timelockActive() == true);
```

### 2.4 部署检查清单

- [ ] 合约部署成功
- [ ] `configure` 完成，参数正确
- [ ] `configureRateLimits` 完成，参数合理
- [ ] relayer 全部添加，数量 >= 3
- [ ] 初始流动性注入，余额 >= `minimumReserve`
- [ ] `activateTimelock` 已执行
- [ ] 对端链桥合约同步完成配置
- [ ] relayer 服务已启动并开始监听事件
- [ ] 监控告警系统已上线

---

## 3. Timelock 操作规范

Timelock 激活后，以下函数需要先调度再执行：

`configure`、`configureRateLimits`、`withdrawToken`、`withdrawETH`、
`addRelayer`、`removeRelayer`、`rotateRelayer`、`proposeAdmin`、
`setGuardian`、`setOperator`、`setRecovery`

### 3.1 调度 → 等待 → 执行

以 `addRelayer` 为例：

```solidity
// 第 1 步：构造操作数据并调度（admin 多签发起）
bytes memory data = abi.encode("addRelayer", 0xNewRelayer...);
bridge.scheduleOperation(data);
// 发出 OperationScheduled 事件，记录 opHash 和 eta

// 第 2 步：等待 24 小时
// 在此期间社区和 guardian 可以审查此操作

// 第 3 步：24h 后执行（48h 内必须执行，否则过期）
bridge.addRelayer(0xNewRelayer...);
// 自动消费 timelock，发出 OperationExecuted 事件
```

### 3.2 取消操作

发现调度的操作有问题时：

```solidity
bytes memory data = abi.encode("addRelayer", 0xSuspiciousAddr...);
bytes32 opHash = keccak256(data);
bridge.cancelOperation(opHash);
```

> `cancelOperation` **不受 pause 限制**，即使合约被冻结也可以取消。

### 3.3 时间窗口

```
调度                  可执行窗口开始           过期
  |                        |                    |
  |---- 24h DELAY -------->|--- 48h GRACE ----->|
  |                        |                    |
  t                    t + 24h             t + 72h
```

- `t` 到 `t + 24h`：等待期，不可执行
- `t + 24h` 到 `t + 72h`：执行窗口，可以执行
- `t + 72h` 之后：已过期，需重新调度

### 3.4 Timelock 数据编码规范

每个函数有固定的编码格式。调度时 `data` 必须与执行时合约内部计算的哈希一致：

| 函数 | abi.encode 格式 |
|------|----------------|
| `configure` | `("configure", usdcAddress, peerContract, localChainId, peerChainId)` |
| `configureRateLimits` | `("configureRateLimits", maxPerWindow, windowDuration, maxSingle, maxStake, minReserve)` |
| `addRelayer` | `("addRelayer", relayerAddress)` |
| `removeRelayer` | `("removeRelayer", relayerAddress)` |
| `rotateRelayer` | `("rotateRelayer", oldRelayer, newRelayer)` |
| `proposeAdmin` | `("proposeAdmin", newAdmin)` |
| `setGuardian` | `("setGuardian", guardianAddress)` |
| `setOperator` | `("setOperator", operatorAddress)` |
| `setRecovery` | `("setRecovery", recoveryAddress)` |
| `withdrawToken` | `("withdrawToken", token, amount, to)` |
| `withdrawETH` | `("withdrawETH", to)` |

---

## 4. 日常运维

### 4.1 监控指标

| 指标 | 告警阈值 | 响应动作 |
|------|----------|----------|
| 金库 USDC 余额 | < `minimumReserve` × 1.5 | 补充流动性 |
| 窗口使用率 | > 80% of `maxUnlockPerWindow` | 关注是否异常 |
| 单笔大额 unlock | > `maxSingleUnlock` × 50% | 记录并审查 |
| relayer 确认延迟 | 某 nonce 超过 10 分钟无确认 | 检查 relayer 健康 |
| 异常 OperationScheduled | 任何非预期的调度 | 立即评估是否需要冻结 |
| EmergencyFreezeActivated | 任何触发 | 通知所有相关方 |

### 4.2 relayer 健康检查

定期检查：

```solidity
uint256 count = bridge.getRelayerCount();
for (uint256 i = 0; i < count; i++) {
    address r = bridge.relayers(i);
    // 检查 r 的 ETH 余额（需要 gas 费）
    // 检查 r 最近的 confirmEvent 交易记录
}
```

如果某个 relayer 长时间不响应：
1. 检查 relayer 服务是否在线
2. 检查 relayer 地址是否有足够 ETH 支付 gas
3. 如果 relayer 已失效，走 Timelock 流程替换：`rotateRelayer(oldRelayer, newRelayer)`

### 4.3 relayer 被入侵

如果怀疑某个 relayer 的私钥泄露：

1. 立即调度 `removeRelayer(compromisedRelayer)` 或 `rotateRelayer(compromised, newRelayer)`
2. 检查该 relayer 近期提交的所有 `confirmEvent`，确认数据是否正确
3. 对该 relayer 已确认但未达阈值的 in-flight nonce，评估是否需要 `skipNonce`
4. 24h 后执行移除/替换

> 被移除的 relayer 已提交的确认投票仍然有效（不会被撤回），但不影响安全性——只要多数 relayer 提交了正确数据，错误投票不会达到阈值。

---

## 5. 紧急响应

### 5.1 场景 A：发现可疑交易 / 潜在攻击

**响应时间：分钟级**

```
Guardian 冻结
      ↓
bridge.emergencyFreeze()
      ↓
所有 stake / confirm / unlock 停止
      ↓
评估情况，决定后续
```

```solidity
// Guardian（EOA）立即调用
bridge.emergencyFreeze();
```

冻结后：
- 所有 `whenNotPaused` 函数不可调用（stake, confirmEvent, 所有 admin 操作）
- `cancelOperation` 仍可调用（admin 可取消恶意调度）
- `executeRecovery` 可调用（recovery 可恢复）

### 5.2 场景 B：admin 密钥泄露

**完整时间线：**

```
T+0     攻击者获取 admin 密钥
T+0~    攻击者调度恶意操作（如 withdrawToken）
T+0~    Guardian 检测到异常 OperationScheduled 事件
T+1min  Guardian 调用 emergencyFreeze() 冻结合约
        → 攻击者的调度被冻结，无法执行
T+?     Recovery 调用 executeRecovery(newAdmin, newGuardian?)
        → 更换 admin，解冻合约
T+?     新 admin 调用 cancelOperation 取消攻击者遗留的所有调度
        → 通过 OperationScheduled 事件索引遗留调度的 opHash
T+?     新 admin 更换可能被泄露的其他角色
```

```solidity
// 1. Guardian 冻结
bridge.emergencyFreeze();

// 2. Recovery 恢复（设新 admin，可选换 guardian）
bridge.executeRecovery(
    0xNewAdmin...,     // 新的安全 admin 地址
    0xNewGuardian...   // 可选：新 guardian（传 address(0) 保留当前）
);

// 3. 新 admin 清理遗留调度
// 从 OperationScheduled 事件中找到所有 opHash
bridge.cancelOperation(opHash1);
bridge.cancelOperation(opHash2);
// ...
```

### 5.3 场景 C：guardian 密钥泄露（恶意冻结 DoS）

攻击者反复冻结合约：

```
Guardian 冻结 → Recovery 解冻 → Guardian 再冻结 → ...
```

**解决方案：** Recovery 在恢复时同时替换 guardian，一步打破循环：

```solidity
// Recovery 同时替换 admin 和 guardian
bridge.executeRecovery(
    0xNewAdmin...,
    0xNewGuardian...   // 替换被泄露的 guardian
);
// 旧 guardian 地址立即失去冻结权限
```

### 5.4 场景 D：operator 密钥泄露

**影响范围有限：**
- Operator 只能 `initiateRefund`（发起退款，6h 后才能执行）和 `skipNonce`
- `executeRefund` 受速率限制 + 单笔限额 + 储备金约束
- 攻击者不能把钱退到自己地址（强制退回原始 staker）
- Admin 可在 6h 延迟期内通过 `cancelRefund` 取消恶意退款

**响应：**

```solidity
// 1. Admin 立即取消攻击者发起的所有退款
bridge.cancelRefund(nonce1);
bridge.cancelRefund(nonce2);

// 2. Admin 调度更换 operator（需 24h timelock）
bytes memory data = abi.encode("setOperator", 0xNewOperator...);
bridge.scheduleOperation(data);

// 如果情况紧急，guardian 可以先冻结合约阻止 operator 操作
bridge.emergencyFreeze();
// recovery 恢复后，新 admin 继续走 timelock 换 operator
```

### 5.5 场景 E：recovery 密钥泄露

**影响：** Recovery 单独持有无法操作（需要 guardian 先冻结）。只要 guardian 没被同时泄露，没有直接风险。

**响应：**

```solidity
// Admin 调度更换 recovery（24h timelock）
bytes memory data = abi.encode("setRecovery", 0xNewRecovery...);
bridge.scheduleOperation(data);
// 24h 后执行
bridge.setRecovery(0xNewRecovery...);
```

---

## 6. Nonce 异常处理（skipNonce + 两步退款）

### 6.1 什么时候需要 skipNonce + 退款

| 场景 | 源链操作 | 目标链操作 |
|------|----------|------------|
| 目标链 receiver 被 USDC 黑名单 | initiateRefund → executeRefund | skipNonce |
| 跨链消息丢失（relayer 全部故障） | initiateRefund → executeRefund | skipNonce |
| 目标链配置变更导致校验失败 | initiateRefund → executeRefund | skipNonce |
| 用户误操作（发到错误地址） | 无法处理 | 无法处理 |

### 6.2 操作顺序（⚠️ 关键）

```
⚠️ 必须严格按此顺序，否则存在双花风险！

第 1 步：在【目标链】skipNonce           — 封死 unlock 可能
第 2 步：在【源链】initiateRefund        — operator 发起退款（记录时间戳）
第 3 步：等待 6 小时（REFUND_DELAY）
第 4 步：在【源链】executeRefund         — operator 或 staker 执行退款
```

**为什么需要 6h 延迟？** 防止 operator 密钥泄露后立即双花。延迟期间 admin 可通过 `cancelRefund` 取消恶意退款。

**为什么顺序重要？** 如果先退款再 skipNonce，在两个操作之间的时间窗口内，relayer 可能在目标链完成 unlock，导致用户同时拿到退款和 unlock（双花）。

### 6.3 操作步骤

**目标链（接收方）—— operator 执行 skipNonce：**

```solidity
// 确认该 nonce 尚未被处理
(bool isProcessed, , ) = bridge.nonceConfirmations(nonce);
assert(!isProcessed);

// 跳过该 nonce
bridge.skipNonce(nonce);

// 确认已跳过
(isProcessed, , ) = bridge.nonceConfirmations(nonce);
assert(isProcessed);
```

**源链（发送方）—— operator 发起退款：**

```solidity
// 确认 stake 记录存在且未退款
(address owner, uint64 amount, bool refunded) = bridge.stakes(nonce);
assert(owner != address(0));
assert(amount > 0);
assert(!refunded);

// 第 1 步：发起退款（记录时间戳）
bridge.initiateRefund(nonce);

// 等待 6 小时...

// 第 2 步：执行退款（operator 或原始 staker 均可调用）
bridge.executeRefund(nonce);

// 确认已退款
(, , bool isRefunded) = bridge.stakes(nonce);
assert(isRefunded);
```

**admin 取消恶意退款：**

```solidity
// 如果发现退款不合理，admin 在 6h 内取消
bridge.cancelRefund(nonce);
```

### 6.4 批量处理

executeRefund 受速率限制约束（与 unlock 共享窗口额度）。批量退款时：
- 在低峰期执行，避免占用正常 unlock 额度
- 分批处理，每批不超过窗口额度
- 如果需要大量退款，可临时调高速率限制（需 Timelock 流程）

---

## 7. 配置变更

### 7.1 调整速率限制

**场景：** 业务增长需要提高限额 / 发现限额过高需要降低

```solidity
// 1. 调度
bytes memory data = abi.encode(
    "configureRateLimits",
    uint64(50_000e6),   // 新窗口限额
    uint64(3600),       // 窗口时长
    uint64(20_000e6),   // 新单笔限额
    uint64(20_000e6),   // 新 stake 限额
    uint64(10_000e6)    // 新最低储备金
);
bridge.scheduleOperation(data);

// 2. 24h 后执行
bridge.configureRateLimits(50_000e6, 3600, 20_000e6, 20_000e6, 10_000e6);
```

> `configureRateLimits` 会自动重置窗口状态（`currentWindowUsage`、`previousWindowUsage`），避免降低限额后因旧用量超过新限额而卡死。

### 7.2 更换 USDC 地址 / 对端合约

⚠️ **高风险操作**。修改 `peerContract` 或链 ID 会导致所有 in-flight 的 `NonceConfirmation` 因校验不匹配而永久卡住。

**操作前必须：**
1. 暂停所有 relayer 服务
2. 等待所有 in-flight nonce 处理完毕（unlock 或 skipNonce）
3. 确认没有待处理的 nonce

```solidity
// 1. 调度
bytes memory data = abi.encode(
    "configure",
    0xNewUSDC...,
    newPeerContract,
    uint64(1),
    uint64(2)
);
bridge.scheduleOperation(data);

// 2. 24h 后执行
bridge.configure(0xNewUSDC..., newPeerContract, 1, 2);

// 3. 恢复 relayer 服务
```

---

## 8. 管理员转移

采用两步转移模式，防止误转到无人控制的地址：

```solidity
// 第 1 步：当前 admin 调度提议（需 24h timelock）
bytes memory data = abi.encode("proposeAdmin", 0xNewAdmin...);
bridge.scheduleOperation(data);

// 第 2 步：24h 后执行提议
bridge.proposeAdmin(0xNewAdmin...);

// 第 3 步：新 admin 接受（无需 timelock）
// 由 0xNewAdmin... 调用
bridge.acceptAdmin();
```

> 新 admin 地址不能与 guardian、operator、recovery 重叠，`acceptAdmin` 会自动校验。

**取消提议：** 如果 `proposeAdmin` 已执行但新 admin 不应接受，admin 需要再调度一次 `proposeAdmin` 指向另一个安全地址来覆盖 `pendingAdmin`。

---

## 9. 流动性管理

### 9.1 补充流动性

直接向桥合约地址转入 USDC：

```solidity
usdc.transfer(address(bridge), amount);
```

无需调用合约函数，ERC20 transfer 即可。

### 9.2 提取流动性

通过 `withdrawToken`（受 Timelock 保护）：

```solidity
// 1. 调度
bytes memory data = abi.encode("withdrawToken", usdcAddress, amount, adminAddress);
bridge.scheduleOperation(data);

// 2. 24h 后执行
bridge.withdrawToken(usdcAddress, amount, adminAddress);
```

> `withdrawToken` 不检查 `minimumReserve`。admin 有责任确保提取后余额仍足以支撑正常运营。

### 9.3 提取误转的 ETH

```solidity
// 1. 调度
bytes memory data = abi.encode("withdrawETH", adminAddress);
bridge.scheduleOperation(data);

// 2. 24h 后执行（提取全部 ETH 余额）
bridge.withdrawETH(payable(adminAddress));
```

### 9.4 提取误转的其他代币

同 `withdrawToken`，将 `token` 参数设为对应代币合约地址。

---

## 10. 桥退役

当需要永久关闭桥时：

**第 1 步：停止新业务**

```solidity
// Guardian 冻结合约
bridge.emergencyFreeze();
```

**第 2 步：处理所有 in-flight nonce**

对每个未处理的 nonce：
- 目标链：`skipNonce(nonce)`
- 源链：`initiateRefund(nonce)` → 等待 6h → `executeRefund(nonce)`

**第 3 步：Recovery 解冻（以便提取资金）**

```solidity
bridge.executeRecovery(adminAddress, address(0));
```

**第 4 步：提取所有资金**

```solidity
// 调度
bytes memory data = abi.encode("withdrawToken", usdcAddress, usdc.balanceOf(address(bridge)), adminAddress);
bridge.scheduleOperation(data);

// 24h 后执行
bridge.withdrawToken(usdcAddress, usdc.balanceOf(address(bridge)), adminAddress);
```

**第 5 步：再次冻结合约**

```solidity
bridge.emergencyFreeze();
// 合约永久处于冻结状态，不再接受任何操作
```

---

## 11. 常见错误排查

### 合约交互错误

| 错误 | 原因 | 解决方案 |
|------|------|----------|
| `Unauthorized` | 调用者不是所需角色 | 检查 msg.sender 是否为对应角色 |
| `EnforcedPause` | 合约处于冻结状态 | 需要 recovery 调用 `executeRecovery` 解冻 |
| `UsdcNotConfigured` | 未调用 `configure` 设置 USDC 地址 | admin 调用 `configure` |
| `TimelockNotScheduled` | Timelock 激活后未调度就执行 | 先 `scheduleOperation`，24h 后执行 |
| `TimelockNotReady` | 24h 等待期未到 | 等待至 eta 时间 |
| `TimelockExpired` | 超过 72h 执行窗口 | 重新 `scheduleOperation` |
| `RateLimitExceeded` | 窗口额度已用完 | 等待窗口滑动，或调整限额 |
| `SingleTransferExceeded` | 单笔金额超限 | 拆分为多笔较小的交易 |
| `InsufficientReserve` | 金库余额不足 | 补充流动性或降低 `minimumReserve` |
| `AlreadyProcessed` | nonce 已处理（重放） | 正常情况，无需处理 |
| `RelayerAlreadyConfirmed` | 同一 relayer 重复确认 | 正常情况，relayer 去重逻辑 |
| `InvalidSourceContract` | 事件数据的源合约地址不匹配 | 检查 relayer 配置是否指向正确的对端合约 |
| `InvalidReceiver` | receiver bytes32 高位非零 | 检查地址格式，EVM 地址应右对齐 |
| `RoleOverlap` | 新角色地址与其他角色重叠 | 使用不同的地址 |
| `ZeroAddress` | 传入了零地址 | 检查参数 |
| `StakeAmountExceeded` | stake 金额超过 `maxStakeAmount` | 减小 stake 金额或调高限额 |
| `NonceAlreadyUsed` | stake 使用了已存在的 nonce | 客户端生成新的随机 nonce |
| `RefundNotInitiated` | 未先 `initiateRefund` 就调用 `executeRefund` | 先调用 `initiateRefund` |
| `RefundNotReady` | 6h 延迟未到就调用 `executeRefund` | 等待 REFUND_DELAY（6h）后执行 |
| `RefundAlreadyInitiated` | 对已发起退款的 nonce 重复发起 | 等待执行或 admin 取消后重试 |

### 跨链问题

| 问题 | 诊断 | 解决 |
|------|------|------|
| 用户 stake 后对端迟迟不 unlock | 检查 relayer 是否正常、是否已提交 confirmEvent | 等待 relayer 确认；如果 relayer 故障，修复后重启 |
| unlock 因 USDC 黑名单失败 | confirmEvent 在达到阈值时 revert | 目标链 `skipNonce` + 源链 `initiateRefund` → `executeRefund` |
| unlock 因速率限制失败 | `RateLimitExceeded` 错误 | 等待窗口滑动，relayer 自动重试 |
| unlock 因储备金不足失败 | `InsufficientReserve` 错误 | 补充流动性 |
| nonce 卡住无法推进 | 检查 `nonceConfirmations` 的投票进度 | 确认是否有足够 relayer 在线；必要时 `skipNonce` + `initiateRefund`/`executeRefund` |

---

## 附录 A：完整函数权限表

| 函数 | 角色 | 需暂停？ | Timelock | 防重入 |
|------|------|---------|----------|--------|
| `activateTimelock` | admin | 否 | — | — |
| `scheduleOperation` | admin | 否 | — | — |
| `cancelOperation` | admin | **不需要** | — | — |
| `configure` | admin | 否 | ✅ | — |
| `configureRateLimits` | admin | 否 | ✅ | — |
| `addRelayer` | admin | 否 | ✅ | — |
| `removeRelayer` | admin | 否 | ✅ | — |
| `rotateRelayer` | admin | 否 | ✅ | — |
| `proposeAdmin` | admin | 否 | ✅ | — |
| `acceptAdmin` | pendingAdmin | 否 | — | — |
| `setGuardian` | admin | 否 | ✅ | — |
| `setOperator` | admin | 否 | ✅ | — |
| `setRecovery` | admin | 否 | ✅ | — |
| `withdrawToken` | admin | 否 | ✅ | ✅ |
| `withdrawETH` | admin | 否 | ✅ | ✅ |
| `emergencyFreeze` | guardian | **仅未暂停** | — | — |
| `executeRecovery` | recovery | **仅已暂停** | — | — |
| `skipNonce` | operator | 否 | — | — |
| `initiateRefund` | operator | 否 | — | — |
| `executeRefund` | operator / staker | 否 | — | ✅ |
| `cancelRefund` | admin | **不需要** | — | — |
| `stake` | 任何人 | 否 | — | ✅ |
| `confirmEvent` | relayer | 否 | — | ✅ |

> "否" = 需要 `whenNotPaused`（合约冻结时不可调用）

---

## 附录 B：事件监控清单

### 必须监控（告警级别：高）

| 事件 | 含义 | 响应 |
|------|------|------|
| `EmergencyFreezeActivated` | 合约被冻结 | 立即排查原因，准备 recovery |
| `OperationScheduled` | 新操作被调度 | 审查内容，确认是否为预期操作 |
| `RecoveryExecuted` | Recovery 介入更换 admin | 确认是否为授权操作 |
| `AdminTransferAccepted` | admin 权限已交接 | 确认新 admin 身份 |

### 应当监控（告警级别：中）

| 事件 | 含义 | 响应 |
|------|------|------|
| `TokensUnlocked` | 代币解锁 | 记录并核对金额 |
| `RefundInitiated` | 退款已发起（6h 后可执行） | 审查退款原因，如异常则 `cancelRefund` |
| `Refunded` | 退款执行完成 | 确认退款原因 |
| `RefundCancelled` | 退款被 admin 取消 | 确认是否为响应安全事件 |
| `NonceSkipped` | nonce 被跳过 | 确认是否有对应退款 |
| `RelayerAdded` / `RelayerRemoved` | relayer 变更 | 确认阈值仍安全 |
| `RateLimitsConfigured` | 速率限制变更 | 审查新参数是否合理 |
| `GuardianUpdated` / `OperatorUpdated` / `RecoveryUpdated` | 角色变更 | 确认新地址 |

### 建议监控（告警级别：低）

| 事件 | 含义 | 响应 |
|------|------|------|
| `StakeEvent` | 用户发起跨链 | 统计业务量 |
| `EventConfirmed` | relayer 提交确认 | 监控 relayer 活跃度 |
| `OperationExecuted` | 调度操作已执行 | 记录合规审计 |
| `OperationCancelled` | 调度操作被取消 | 记录原因 |
