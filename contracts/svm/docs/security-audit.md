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
| 手续费 | `bridgeFee` stake 时扣除 | `bridge_fee` stake 时扣除 | 两端一致：源链 stake 扣费，目标链 unlock 全额转给用户 |
| Nonce | 客户端随机 + mapping 校验唯一性 | 客户端随机 + PDA init 天然防碰撞 | 两端一致：随机 nonce 防 skip_nonce DoS |
| Refund | 两步：`initiateRefund` → 6h → `executeRefund` | 两步：`initiate_refund` → 6h → `execute_refund` | 两端一致：operator 或 staker 可执行第 2 步 |
| Refund 取消 | `cancelRefund`（admin，暂停时可用） | `cancel_refund`（admin，暂停时可用） | 两端一致 |
| PDA 压缩 | N/A | compact 缩小 + 退还租金 | SVM 特有，减少链上存储成本 |
| Token 标准 | ERC-20 (`safeTransfer`) | token_interface (`transfer_checked`) | SVM 兼容 Token-2022 |
| 事件哈希 | `keccak256(abi.encode(...))` | `SHA-256(hashv(...))` | 哈希算法不同，字节布局通过固定宽度对齐 |
| `cancel_operation` 暂停限制 | 无 `whenNotPaused` | 无 `is_paused` 约束 | 两端一致：暂停时可取消已调度操作 |

### 剩余 SVM 独有错误码及其原因

在 R6 错误码精简之后（删除 `PeerNotConfigured` / `NonceMismatch` / `InvalidEventData` / `RequestNotCompleted` / `NonceOverflow` / `InsufficientBalance`，hub 同步删除 `SourceChainIdMismatch`），SVM 端仍保留 3 个 EVM 不存在的错误码，每一项均为结构性必需：

| 错误码 | 仅 SVM 的原因 |
|--------|---------------|
| `ReceiverMintMismatch` | 在 account validation 阶段拦截非 USDC mint 的 `receiver_token_account`，防止脏数据进入投票池导致 nonce 卡死。EVM 没有 token-account 概念，无对应需求。 |
| `Paused` | 与 OZ Pausable v5 的 `EnforcedPause()` 功能等价，仅命名风格差异。SVM 由 `is_paused` 布尔位 + 构造检查实现，保留现名便于诊断。 |
| `NotPaused` | 与 OZ Pausable v5 的 `ExpectedPause()` 对应（`execute_recovery` 要求处于暂停态）；保留 SVM 现名便于诊断。 |

### EVM 有 / SVM 没有的错误码

| EVM 错误码 | SVM 缺失原因 |
|------------|-------------|
| `ETHTransferFailed` | 本轮不做 `withdraw_sol`，无消费场景。 |
| `AuthBindingMismatch` | gasless EIP-3009 路径本轮不动；EVM 中由 EIP-712 binding 校验，SVM `stake_gasless` 走 fee-payer 模型不需要。 |
| `NonceAlreadyUsed` | SVM 由 PDA `init` 失败自动兜底（`Anchor: "account already in use"`），不需要独立错误码。 |

### R6 链下 follow-up：IDL 变更影响的调用方

本轮接受了 IDL 破坏性变更（`configure` 重排参数、`confirm_event` 只收 `event_data`、`schedule_operation` 只收 `data`），需同步调整 `[1024-bridge/relayer]` 与 `[1024-bridge/deploy]` 中以下调用方（本轮不修，单独 PR 处理）：

| 文件 | 行号 | 当前调用 | 需要的更新 | 影响目标 |
|------|------|----------|------------|----------|
| `relayer/src/svm/submitter.rs` | 262–266 | ix_data 拼 `[disc][8B nonce LE][8B source_chain_id LE][Borsh(BridgeEventData)]` | 去掉 nonce + source_chain_id 这 16 字节，只保留 `[disc][Borsh(BridgeEventData)]`（两个字段已经在 `BridgeEventData` 里） | hub `confirm_event` |
| `deploy/svm/src/instructions/role-op.ts` | 111 | `program.methods.scheduleOperation(Array.from(opHash), Array.from(data))` | 改为 `program.methods.scheduleOperation(Buffer.from(data))`；`opHash` 仍由链下计算用于派生 `timelockOpPda` | hub & leaf `schedule_operation` |
| `deploy/svm/src/instructions/configure.ts` | 30 | `program.methods.configure(usdcMint, localChainId)` | **无需修改**：hub `configure` 签名仍是 `(usdc_mint, local_chain_id)`，本轮重排只动 leaf。该脚本仅服务 hub。 | hub `configure`（无变更） |

leaf `configure` 当前在 `deploy/svm` 目录下没有专属调用脚本（多 peer 部署通过 hub 路径 + `register_peer` 完成），故不需要 leaf-side 的 deploy 脚本改动。

`deploy/ui/src/ops/svm-configure.mjs` 与 `svm-manage-roles.mjs` 都是 `ts-node` 包装层，本身不直接调用 IDL，跟随 `configure.ts` / `role-op.ts` 改动即可。

`relayer/src/evm/submitter.rs` 的 `confirmEvent` 选择器使用的是固定字符串 `confirmEvent((bytes32,...,uint64))` —— **EVM 端签名未动**，无需变更。

---

## 第 5 轮审计补丁 (R5)

本轮与 EVM 端 R5 对齐，聚焦"角色治理边界"与"转出限额职责切分"。全部已落地，对应 TypeScript 测试并入 `tests/bridge1024.ts`，`anchor test` 47 / 47 通过。

---

### R5-INV1: refund 路径不应受 `max_single_unlock` 约束 — "能 stake 必须能 refund" 不变量

**位置**: `programs/bridge1024/src/helpers.rs` — `check_dual_transfer_limits()` / `check_global_transfer_limits()`

**风险**: 原实现中 `check_dual_transfer_limits` 与 `check_global_transfer_limits` 同时被 `confirm_event`（unlock）和 `execute_refund`（退款）调用，并在内部重复校验 `amount > max_single_unlock`。与 EVM 端相同的不变量违反：用户在 stake 阶段通过了 `max_stake_amount`，事后 admin 把 `max_single_unlock` 调低，`execute_refund` 就会被永久卡住，用户资金锁死在 PDA。

`max_single_unlock` 在语义上属于"对端来的解锁到账"的早拒防御，不应反向作用于用户自己的退款；`max_stake_amount` 已在 stake 阶段做过单笔上限。

**修复**:

1. `confirm_event` 入口把 per-chain 与全局的 `max_single_unlock` 以 `saturating_sub(bridge_fee)` 后的 `preview_net` 提前早拒（见 R5-L2）；
2. `check_dual_transfer_limits` / `check_global_transfer_limits` 内部仅保留**滑动窗口速率限制**（per-chain + 全局两层），移除 `max_single_unlock` 重复检查；
3. `execute_refund` 现在只走速率限制与金库不变量，`max_single_unlock` 的变化不会阻断历史 stake 的回退。

因 SVM 端的 refund 路径过去就没有单独覆盖 "max_single_unlock 阻断 refund" 的回归测试，此次不变量修复的正确性通过 EVM 端的 `testRefund_NotBlockedByMaxSingleUnlock` / `testConfirmEvent_StillBoundByMaxSingleUnlock` 镜像验证，并在 SVM 端确保 `confirm_event` 仍在 peer_config / bridge_state 两层的 `max_single_unlock` 上正确早拒。

**状态**: 🟢 已修复

---

### R5-M1: `propose_admin` 与 `set_{guardian,operator,recovery}` 缺少角色重叠预检

**位置**: `programs/bridge1024/src/lib.rs` — `propose_admin()` / `set_guardian()` / `set_operator()` / `set_recovery()`

**风险**: 与 EVM 端 R5-M1 完全对称：

1. `propose_admin(new_admin)` 在 24h Timelock 调度期只校验 `new_admin != Pubkey::default()`，直到 `accept_admin` 阶段才做 `new_admin ∉ {guardian, operator, recovery}` 校验。若误提议为已有角色，Timelock schedule 白费且 `pending_admin` 卡死；
2. `set_guardian / set_operator / set_recovery` 的 `RoleOverlap` 检查里漏了 `!= pending_admin`，会导致已提议的 `pending_admin` 在 `accept_admin` 阶段卡死。

**修复**:

1. `propose_admin` 增加预检 `new_admin ∉ {admin, guardian, operator, recovery}`；
2. 三个 `set_*` 在 `RoleOverlap` 分支里追加 `!= pending_admin`；
3. `execute_recovery` 保持原有的 `pending_admin = Pubkey::default()` 收尾。

新增测试：
- `test_propose_admin_rejects_overlap_with_admin_guardian_operator_recovery`：覆盖 4 种已占用角色的预检；
- `test_set_guardian_operator_recovery_reject_overlap_with_pending_admin`：分别验证三个 setter 与 `pending_admin` 的重叠拒绝。

**状态**: 🟢 已修复

---

### R5-L2: `confirm_event` 阶段未提前校验 `max_single_unlock` — 浪费 relayer CU

**位置**: `programs/bridge1024/src/lib.rs` — `confirm_event()`

**风险**: 与 EVM R5-L2 同构，原实现在阈值达成、即将 unlock 时才通过 `check_dual_transfer_limits` 做 `max_single_unlock` 校验。所有 relayer 都会成功投票直到最后一个触发限额 revert，`EventVote` PDA 状态被污染、CU 被浪费。

**修复**: 在 `confirm_event` 入口立即用 `event_data.amount` 比较 `max_single_unlock`（源链已扣费，此处无需再减），同时覆盖 peer-chain 与全局两层：

```rust
let preview_net = event_data.amount.saturating_sub(pc.bridge_fee);
if pc.max_single_unlock != 0 && preview_net > pc.max_single_unlock {
    return err!(ErrorCode::SingleTransferExceeded);
}
if bs.max_single_unlock != 0 && preview_net > bs.max_single_unlock {
    return err!(ErrorCode::SingleTransferExceeded);
}
```

配合 R5-INV1 把 helpers 内部的 `max_single_unlock` 重复检查移除，使得整个程序里 `max_single_unlock` 只在 `confirm_event` 入口的一个点表达语义。

**状态**: 🟢 已修复

---

### R5-L3: `_target_chain_id` / `_source_chain_id` 在 IDL 中不可见

**位置**: `programs/bridge1024/src/lib.rs` — `stake()` / `skip_nonce()`

**风险**: 原实现对 `target_chain_id` / `source_chain_id` 使用 `_` 前缀（Rust 约定抑制 unused warning），PDA seeds 已隐式约束链 ID 一致性，**但这层约束不会在 Anchor IDL 中以参数语义的形式出现**。客户端（relayer、用户钱包、审计工具）通过 IDL 构造交易时无法直接看到这两个参数、也无法静态校验；语义完全依赖开发者熟读 seeds 逻辑。

**修复**:

1. 去掉 `_target_chain_id` / `_source_chain_id` 的下划线，让参数在 IDL 中显式出现；
2. `stake` 增加 `require!(target_chain_id == pc.chain_id, ErrorCode::InvalidChainId)` 作为防御性断言（消除 unused 警告，同时提供比 "ConstraintSeeds" 更语义化的错误码）；
3. `skip_nonce` 的 `source_chain_id` 不再写入 `msg!` 日志（`msg!` 与 tx instruction data 信息重复、易被 RPC 裁剪），改为：
   - 在 `NonceSkipped` 事件里新增 `source_chain_id` 字段，给链下索引器区分不同对端链的 nonce 空间；
   - `CrossChainRequest` state 不存储 `source_chain_id`，其一致性由 PDA seeds 在 Anchor seeds 派生阶段强制，不需要 instruction 内再做冗余 `require!`。

新增测试 `test_stake_works_after_target_chain_id_rename_and_matches_peer_config_chain_id` 验证 stake 在 IDL 可见参数下正常工作，并确认与 `peer_config.chain_id` 的一致性约束。

**状态**: 🟢 已修复

---

## 第 6 轮审计补丁 (R6)

R6 错误码精简的相关说明见上文「与 EVM 端的差异对照 → R6 链下 follow-up」。本节追加 R6 阶段两项第三方审计后的小修补，均不改变 IDL 表面。

---

### R6-L1: `do_stake` 的 `total_fee` 使用 `saturating_add` 与项目防御编码风格不一致

**位置**: `programs/bridge1024/src/lib.rs` — `do_stake()`

**风险**: leaf 的 `do_stake` 在 gasless 路径下计算 `total_fee = bridge_fee + gasless_fee` 时使用 `saturating_add`：

```rust
// 修复前：
let total_fee = if gasless {
    bs.bridge_fee.saturating_add(bs.gasless_fee)
} else {
    bs.bridge_fee
};
```

`bridge_fee` 与 `gasless_fee` 都在各自的 `configure_*` 入口受 `MAX_FEE = 1e9` 兜底，二者之和 ≤ 2e9 远不会触顶 u64，**当前实现不存在实际溢出风险**。但此处的 `saturating_add` 与项目其他位置（`consume_timelock` 的 lamports 加法、`compact_request_pda` 的退款累加，参考 L-4）一致采用的 `checked_add + ok_or` 风格不一致；如果未来调高 `MAX_FEE` 或引入新的费用项，悄悄饱和到 `u64::MAX` 会让随后的 `checked_sub` 把语义错误地呈现为普通的 `FeeExceedsAmount`，掩盖真正的配置错误。

**修复**: 改为 `checked_add` 并映射到 `FeeExceedsAmount`：

```rust
let total_fee = if gasless {
    bs.bridge_fee
        .checked_add(bs.gasless_fee)
        .ok_or_else(|| error!(ErrorCode::FeeExceedsAmount))?
} else {
    bs.bridge_fee
};
```

行为等价（正常配置下永远不会进溢出分支），但意图固化下来；与 EVM 端 Solidity 0.8 默认的 checked 算术行为也保持一致。

**状态**: 🟢 已修复（防御性，无功能影响）

---

### R6-M1: hub 的 `skip_nonce` 应显式拒绝 `source_chain_id == local_chain_id`

**位置**: `programs/bridge1024_hub/src/lib.rs` — `skip_nonce()` + `src/contexts.rs` — `SkipNonce`

**风险**: hub 的 `SkipNonce` 上下文**有意**不要求 `PeerConfig` 账户（注释中说明这是为了在 `unregister_peer` 之后仍能清理遗留 nonce），由此带来一个副作用：operator 可以传入任意 `source_chain_id` 创建 `CrossChainRequest` PDA 并标记 `is_processed = true`，**包括传入 `local_chain_id`**。

虽然合约本身不可能"从自己向自己"跨链（`register_peer` 在注册阶段就拒绝 `chain_id == local_chain_id`），但此空间没有任何防御：

1. operator 误操作 / 脚本 bug 时，PDA 会被永久占位，未来无法被回收（leaf / hub 都不允许关闭 `CrossChainRequest`）
2. operator 被攻陷时可自费消耗租金 DoS 本链 nonce 空间（虽然 nonce 是随机 u64、空间够大，但属于不必要的攻击面）

**修复**: 在 `skip_nonce` 指令体内显式拒绝 `source_chain_id == local_chain_id`，复用已有的 `InvalidChainId` 错误码、不引入新 error、不改 IDL：

```rust
require!(
    source_chain_id != ctx.accounts.bridge_state.local_chain_id,
    ErrorCode::InvalidChainId
);
```

为什么不放在 `SkipNonce` 账户约束里：`SkipNonce` 上下文必须容忍 PeerConfig 不存在（为支持 `unregister_peer` 之后的清理），所以 chain_id 校验只能放在指令体内。leaf 的 `skip_nonce` 不需要这项校验——leaf 的 `CrossChainRequest` PDA seeds 只用 nonce，根本没有 `source_chain_id` 参数。

**状态**: 🟢 已修复（结构性收紧，无功能影响）
