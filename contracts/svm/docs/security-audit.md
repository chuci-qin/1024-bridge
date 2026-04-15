# Bridge1024 SVM 合约安全审计

## 高危 (High)

### H-1: `initialize` 可被任意用户抢先调用 — 部署后前置交易夺取 admin 权限

**位置**: `src/lib.rs` — `initialize()` + `src/contexts.rs` — `Initialize`

```rust
#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = admin,
        space = BridgeState::LEN,
        seeds = [b"bridge_state"],
        bump,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(mut)]
    pub admin: Signer<'info>,
    // ...
}
```

**风险**: BridgeState PDA 使用固定种子 `[b"bridge_state"]`，`initialize` 不验证调用者身份。Solana 的标准部署流程中，程序部署（BPF Loader finalize）和首次调用是分开的交易，攻击者可在部署完成后立即发送 `initialize`，将自己设为 admin、guardian、operator、recovery，完全控制桥合约。

**攻击流程**:

1. 攻击者监控链上新程序部署
2. 检测到 bridge1024 程序 ID 出现后，立即构造 `initialize` 交易
3. 设置四个角色为攻击者控制的地址
4. 攻击者成为唯一管理者，可配置桥参数、添加恶意中继器、窃取所有跨链资金

**建议**: 硬编码预期 admin 地址，或验证调用者是程序的 upgrade authority。

**状态**: 🟢 已修复 — 在程序中硬编码 `INITIAL_ADMIN` 常量，`initialize` 入口验证 `admin.key() == INITIAL_ADMIN`。相比 `verify_upgrade_authority` 方案，硬编码方式无 Solana 版本兼容性问题（`solana-test-validator` 在 Solana 3.x 中改变了 `--upgradeable-program` 参数格式，导致 ProgramData 的 upgrade_authority 可能为零地址）

---

## 中危 (Medium)

### M-1: 被移除的 relayer 已提交的确认投票仍然有效

**位置**: `src/lib.rs` — `confirm_event()` 投票机制 + `remove_relayer()`

**风险**: 当一个 relayer 被从白名单移除时，其已提交的确认投票（存储在 `CrossChainRequest.confirmed_relayers` 和 `hash_votes` 中）不会被清除，仍计入阈值。

场景：3 个 relayers (A, B, C)，`frozen_threshold = 2`。Relayer A 对 nonce X 提交确认后被移除（发现已被入侵），Relayer B 随后提交确认 → 投票数达到 2 → 触发 unlock。此时 A 的旧投票仍生效。

**缓解因素**:

- 如果被入侵的 relayer 提交的是错误数据（不同 hash），则不影响正确数据的投票
- Operator 可在移除 relayer 后对相关 in-flight nonce 调用 `skip_nonce` 处理
- 实现 epoch 机制使旧投票失效会显著增加复杂度和 CU 成本

**状态**: 🟡 接受风险 — 通过运维流程（移除 relayer 时同步检查 in-flight nonce）缓解

---

### M-2: `execute_recovery` 不清除已调度的 timelock 操作

**位置**: `src/lib.rs` — `execute_recovery()`

**风险**: 紧急恢复时 `TimelockOperation` PDA 中已调度的操作未被清除。新 admin 可能意外消费旧 admin 遗留的调度：

1. 被攻破的 admin 调度恶意操作，例如 `schedule_operation("addRelayer" || malicious_pubkey)`
2. Guardian 检测到并冻结合约
3. Recovery 设置新 admin 并解冻（遗留调度未被清除）
4. 新 admin 调用相同参数的 `add_relayer(malicious_pubkey)` → `consume_timelock` 发现旧调度的 ETA 已过且在 grace period 内 → 直接执行，跳过 24h 等待

**缓解因素**:

- 攻击者需要精确猜测或影响新 admin 的操作参数
- Grace period 为 72h（24h delay + 48h window），超时后自动失效
- `cancel_operation` 无 `is_paused` 限制，新 admin 可在任何状态下清除遗留调度

**状态**: 🟡 接受风险 — 新 admin 应在执行任何操作前通过 `OperationScheduled` 事件索引遗留调度，并调用 `cancel_operation` 清除

---

### M-3: `execute_refund` 与 `confirm_event` 共享速率限制窗口

**位置**: `src/lib.rs` — `execute_refund()` 和 `confirm_event()` 均调用 `check_transfer_limits()`

**风险**: `execute_refund`（发送端退款）和 `confirm_event` 的 unlock 分支（接收端解锁）共享同一组速率限制计数器（`current_window_usage`、`previous_window_usage`）。在同一链同时扮演发送方和接收方的场景下，大量退款会消耗 unlock 额度，反之亦然。

场景：窗口限额 1000 USDC，operator 在窗口内退款 800 USDC，则同一窗口内只剩 200 USDC 的 unlock 额度。

**缓解因素**: execute_refund 由 operator 或 staker 调用（需 operator 先 initiate_refund 并等待 6h），operator 应当协调操作节奏；在实际部署中，发送端和接收端通常是不同链上的不同合约实例。

**状态**: 🟡 接受风险 — 运维文档中明确提醒 execute_refund 和 unlock 共享额度，operator 应在低峰期发起批量退款

---

### M-4: `StakeRecord` PDA 永远不会关闭 — 用户租金永久锁定

**位置**: `src/contexts.rs` — `StakeAccounts` + `InitiateRefund` / `ExecuteRefund`

```rust
#[account(
    init,
    payer = user,
    space = StakeRecord::LEN,
    seeds = [b"stake_record", nonce.to_le_bytes().as_ref()],
    bump,
)]
pub stake_record: Account<'info, StakeRecord>,
```

**风险**: 每次 `stake` 创建一个 `StakeRecord` PDA（用户支付 ~0.00157 SOL 租金），但无论是对端 unlock 成功还是本端 execute_refund 完成，该 PDA 都**永远不会被关闭**。对于高频用户，租金累积不可忽视。

**建议**: 为 execute_refund 完成后的 `StakeRecord` 添加 `close_stake_record` 指令，允许 owner 回收租金。

**状态**: 🟡 接受风险 — StakeRecord 需要永久保留防止 nonce 重用导致的安全问题；用户可视为跨链手续费的一部分

---

## 低危 (Low)

### L-1: 无最小 relayer 数量约束

**位置**: `src/lib.rs` — `remove_relayer()`

```rust
pub fn remove_relayer(ctx: Context<AdminOp>, relayer: Pubkey) -> Result<()> {
    // ...
    let bs = &mut ctx.accounts.bridge_state;
    let idx = bs.relayers.iter().position(|r| *r == relayer)
        .ok_or(error!(ErrorCode::RelayerNotFound))?;
    bs.relayers.swap_remove(idx);
    // ...
}
```

**风险**: Admin 可以移除所有 relayer（导致桥完全冻结），或将 relayer 减至 1 个（`frozen_threshold = 1`，单点信任）。没有最小数量约束意味着桥的安全性可以被任意降低。

**状态**: 🟡 接受风险 — admin 为多签钱包，具备充分治理保障；`remove_relayer` 受 timelock 保护

---

### L-2: `compact_request_pda` 与 Anchor exit 序列化的精确字节耦合

**位置**: `src/helpers.rs` — `compact_request_pda()` + `src/state.rs` — `CrossChainRequest::COMPACTED_LEN`

```rust
pub fn compact_request_pda<'info>(
    req_info: &AccountInfo<'info>,
    refund_to: &AccountInfo<'info>,
) -> Result<()> {
    let old_lamports = req_info.lamports();
    req_info.resize(CrossChainRequest::COMPACTED_LEN)?;
    // ...
}
```

**风险**: `compact_request_pda` 将账户缩小到 `COMPACTED_LEN`（27 字节），Anchor exit 时重新序列化清空后的 `CrossChainRequest` 恰好填满 27 字节。这种精确字节数匹配非常脆弱——未来任何对 `CrossChainRequest` 字段的修改（即使只增加一个 `bool`）都会导致序列化溢出 panic。

**建议**: 添加编译期静态断言或单元测试，确保 `COMPACTED_LEN` 始终 ≥ 序列化后的最小尺寸。

**状态**: 🟡 接受风险 — 当前版本字节数精确匹配，未来修改时需同步更新 COMPACTED_LEN

---

### L-3: `stake` 中对 `amount` 的早期检查对 fee-on-transfer 代币有误导性

**位置**: `src/lib.rs` — `stake()`

```rust
// 修复前：
require!(amount > bs.bridge_fee, ErrorCode::FeeExceedsAmount);
```

**风险**: 检查的是用户传入的 `amount`（CPI transfer 前），但对于 fee-on-transfer 代币（Token-2022 扩展），实际到账的 `actual_amount` 可能小于 `amount`。用户传入的 `amount` 通过首次检查但 `actual_amount` 不够，最终在后续的 `actual_amount.checked_sub(bridge_fee)` 处失败，得到不够清晰的错误信息。

**状态**: 🟢 已修复 — 将 `require!(amount > bs.bridge_fee)` 改为 `require!(amount > 0, ErrorCode::ZeroAmount)`，仅做零值检查；真正的手续费校验交给 CPI 转账后的 `actual_amount.checked_sub(bridge_fee)` 处理

---

### L-4: `compact_request_pda` 中 lamports 加法无溢出检查

**位置**: `src/helpers.rs` — `compact_request_pda()`

```rust
// 修复前：
**refund_to.try_borrow_mut_lamports()? += refund;
```

**风险**: 与 `consume_timelock` 中使用 `checked_add` 不同，此处直接使用 `+=`。虽然 lamports 实际上几乎不可能溢出 u64，但不一致的处理方式不符合安全编码最佳实践。

**状态**: 🟢 已修复 — 改为 `checked_add(refund).ok_or(ProgramError::ArithmeticOverflow)`，与 `consume_timelock` 风格一致

---

### L-5: `withdraw_token` 可绕过 `minimum_reserve` 保护

**位置**: `src/lib.rs` — `withdraw_token()`

```rust
pub fn withdraw_token(ctx: Context<WithdrawToken>, amount: u64, to: Pubkey) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);
    // ... timelock check ...
    // 直接转账，不调用 check_vault_invariant
    token_interface::transfer_checked(...)?;
}
```

**风险**: `unlock` 和 `execute_refund` 都调用 `check_vault_invariant` 确保金库余额不低于 `minimum_reserve`，但 `withdraw_token` 不做此检查。admin 可通过 timelock 调度一笔提取，将金库 USDC 余额降至 minimum_reserve 以下。

**状态**: 🟡 接受风险 — `withdraw_token` 是流动性管理/桥退役用途，受 24h Timelock 保护，admin（多签）全权负责

---

### L-6: `skip_nonce` 对 nonce 值无范围限制 — operator 可预先封锁未来 nonce

**位置**: `src/lib.rs` — `skip_nonce()` + `src/contexts.rs` — `SkipNonce`

```rust
pub fn skip_nonce(ctx: Context<SkipNonce>, nonce: u64) -> Result<()> {
    let req = &mut ctx.accounts.cross_chain_request;
    require!(!req.is_processed, ErrorCode::AlreadyProcessed);
    req.is_processed = true;
    req.nonce = nonce;
    // ...
}
```

**风险**: `skip_nonce` 不校验 nonce 的合理范围。在旧版自增 nonce 设计下，恶意 operator 可以预先 skip 大量未来 nonce，造成 DoS。

**状态**: 🟢 已修复 — 通过两项改进大幅缩小攻击面：

1. **随机 nonce**：nonce 改为客户端随机生成，攻击者无法预测未来 nonce，只能逐笔竞速 skip 已发生的 stake（难度大幅上升）
2. **两步退款延迟**：`refund` 改为 `initiate_refund` → 等待 6h → `execute_refund`，operator 泄露后无法立即双花，admin 有充足反应窗口取消恶意退款

---

### L-7: `cancel_operation` 在暂停状态下不可调用 `schedule_operation` 但可调用取消

**位置**: `src/contexts.rs` — `ScheduleOperation` vs `CancelOperation`

```rust
// ScheduleOperation 约束：
constraint = !bridge_state.is_paused @ ErrorCode::Paused,

// CancelOperation 约束：无 is_paused 检查
```

**说明**: 这是有意设计——暂停时不能调度新操作，但可以取消已有操作（紧急清理）。已在 EVM 端审计中确认并修复为相同行为。

**状态**: ✅ 设计符合预期

---

## 信息性 (Informational)

### I-1: `is_relayer` 使用 O(n) 数组遍历

**位置**: `src/state.rs` — `BridgeState::is_relayer()`

```rust
pub fn is_relayer(&self, addr: &Pubkey) -> bool {
    self.relayers.iter().any(|r| r == addr)
}
```

**说明**: `confirm_event` 每次调用时线性扫描 relayers 数组。因 `n ≤ 18`（MAX_RELAYERS），CU 差异极小。SVM 不支持 O(1) 的 HashMap 链上存储，替代方案（为每个 relayer 创建 PDA）会增加账户管理复杂度。

**状态**: 🟡 接受 — n ≤ 18 性能可接受

---

### I-2: `confirm_event` 中 `receiver_token_account` 可因不同 relayer 而不同

**位置**: `src/contexts.rs` — `ConfirmEvent` + `src/lib.rs` — `confirm_event()`

```rust
#[account(
    mut,
    constraint = receiver_token_account.mint == usdc_mint.key()
        @ ErrorCode::ReceiverMintMismatch,
)]
pub receiver_token_account: InterfaceAccount<'info, TokenAccount>,
```

**说明**: 每个 relayer 调用 `confirm_event` 时可传入不同的 `receiver_token_account`（只要 owner 匹配 `event_data.receiver`）。实际 unlock 转账使用触发阈值的 relayer 提供的 token account。不构成资金损失（owner 已校验），但资金可能到达用户意料之外的 ATA。

由于所有投票者的 event_data 取 SHA-256 哈希后投票，达到阈值意味着多数 relayer 提交了相同的数据（相同的 receiver），因此 token account 的 owner 校验足以保证安全。

**状态**: 🟡 接受 — owner 校验已覆盖安全需求

---

### I-3: `frozen_threshold` 在首次 `confirm_event` 时冻结

**位置**: `src/lib.rs` — `confirm_event()`

```rust
if req.confirmed_relayers.is_empty() {
    req.nonce = event_data.nonce;
    req.frozen_threshold = ((bs.relayers.len() as u16 * 2 + 2) / 3) as u8;
    req.hash_votes = Vec::new();
}
```

**说明**: 阈值在首次确认时快照冻结，后续 relayer 数量变化不影响进行中的投票。这是正确的设计——防止 admin 通过增减 relayer 操纵投票结果。但也意味着如果首次确认时 relayer 较少（如仅 1 个），阈值会很低。

**状态**: ✅ 设计符合预期

---

### I-4: `init_if_needed` 安全性依赖 PDA 永不关闭

**位置**: `src/contexts.rs` — `ConfirmEvent`、`SkipNonce`

```rust
#[account(
    init_if_needed,
    payer = relayer,
    space = CrossChainRequest::LEN,
    seeds = [b"cross_chain_request", _nonce.to_le_bytes().as_ref()],
    bump,
)]
pub cross_chain_request: Account<'info, CrossChainRequest>,
```

**说明**: `init_if_needed` 的已知风险是"关闭后重建"导致状态重置（双花）。本合约通过**永不关闭** `CrossChainRequest` PDA 来规避此问题——compact 仅缩小账户尺寸，不关闭。处理完成后 `is_processed = true` 的标志位永久保留。

**前提条件**: 合约中不存在关闭 `CrossChainRequest` PDA 的指令。未来如需添加垃圾回收功能，必须确保已处理的 PDA 不可关闭。

**状态**: ✅ 设计正确，前提条件已满足

---

## 五轮审计新增 (Round 5)

### R5-M1: `confirm_event` 中 `bridge_fee` 可在投票过程中被修改

**位置**: `src/lib.rs` — `confirm_event()` + `configure_peer_fee()`

```rust
// 每个 relayer 投票时都读取当前 PeerConfig 的 bridge_fee
require!(event_data.amount > pc.bridge_fee, ErrorCode::FeeExceedsAmount);

// 达到阈值时再次使用当前 bridge_fee 计算净额
let net_amount = event_data.amount
    .checked_sub(pc.bridge_fee)
    .ok_or_else(|| error!(ErrorCode::FeeExceedsAmount))?;
```

**风险**: `bridge_fee` 存储在 `PeerConfig` PDA 中，可通过 `configure_peer_fee`（受 Timelock 保护）修改。如果 admin 在某个 nonce 的投票过程中修改了 `bridge_fee`，会产生以下影响：

1. **fee 提高**：后续 relayer 投票时可能因 `event_data.amount <= new_bridge_fee` 被拒绝，导致无法凑够阈值，nonce 永久卡住（需 `skip_nonce` + refund 流程处理）
2. **fee 降低**：触发阈值的 relayer 使用更低的 fee 计算 `net_amount`，用户实际收到的金额多于原始 fee 下的预期值（协议收入减少）
3. **fee 提高到接近 amount**：即使凑够阈值，`net_amount` 变得极小，用户实际收到几乎为零

**缓解因素**:
- `configure_peer_fee` 受 24h Timelock 保护，不会被即时修改
- 正常运维中 fee 变更极少，且 24h 延迟为进行中的投票提供了充足的完成窗口
- 卡住的 nonce 可通过 `skip_nonce` + `initiate_refund` / `execute_refund` 退还用户资金

**运维要求**: 修改 `bridge_fee` 前，operator 应确认所有进行中的 `CrossChainRequest`（`is_processed == false`）已达到阈值完成解锁，或通过 `skip_nonce` 标记为已处理。可通过链上索引 `EventConfirmed` 事件与 `TokensUnlocked` / `NonceSkipped` 事件的差集识别进行中的 nonce。

**状态**: 🟡 接受风险 — Timelock 的 24h 延迟提供了充足缓冲；运维文档中明确提醒 fee 变更前须清理进行中的投票

---

## 与 EVM 端的差异对照

| 项目 | EVM | SVM | 说明 |
|------|-----|-----|------|
| 初始化保护 | 构造函数（部署即初始化） | `init` PDA + upgrade authority 校验 | SVM 需额外防护部署与初始化分离的时间窗口 |
| 重入保护 | `nonReentrant` modifier | Solana 运行时原生防重入 | SVM 不需要显式 nonReentrant |
| 暂停机制 | OpenZeppelin `Pausable` | `is_paused` 手动管理 | 功能等价 |
| Timelock | `mapping(bytes32 => uint256)` ETA | PDA per operation | SVM 使用 PDA 替代 mapping，消费时手动关闭 |
| 手续费 | 无（仅 EVM gas） | `bridge_fee` 双向扣除 | SVM 特有，stake 和 unlock 时扣除 |
| Nonce | 客户端随机 + mapping 校验唯一性 | 客户端随机 + PDA init 天然防碰撞 | 两端一致：随机 nonce 防 skip_nonce DoS |
| Refund | 两步：`initiateRefund` → 6h → `executeRefund` | 两步：`initiate_refund` → 6h → `execute_refund` | 两端一致：operator 或 staker 可执行第 2 步 |
| Refund 取消 | `cancelRefund`（admin，暂停时可用） | `cancel_refund`（admin，暂停时可用） | 两端一致 |
| PDA 压缩 | N/A | compact 缩小 + 退还租金 | SVM 特有，减少链上存储成本 |
| Token 标准 | ERC-20 (`safeTransfer`) | token_interface (`transfer_checked`) | SVM 兼容 Token-2022 |
| 事件哈希 | `keccak256(abi.encode(...))` | `SHA-256(hashv(...))` | 哈希算法不同，字节布局通过固定宽度对齐 |
| `cancel_operation` 暂停限制 | 无 `whenNotPaused` | 无 `is_paused` 约束 | 两端一致：暂停时可取消已调度操作 |
