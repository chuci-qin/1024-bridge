use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::errors::ErrorCode;
use crate::state::*;

// ─── 初始化 ──────────────────────────────────────────────────────────────────

/// 初始化桥合约的账户上下文。
/// 创建全局 BridgeState PDA 和 vault PDA。
/// 仅可调用一次（PDA 的唯一性由 seeds 保证）。
///
/// 安全性：通过硬编码 INITIAL_ADMIN 地址防止 front-running，无 Solana 版本兼容性问题。
#[derive(Accounts)]
pub struct Initialize<'info> {
    /// 全局桥状态 PDA。init 约束确保只能创建一次。
    #[account(
        init,
        payer = admin,
        space = BridgeState::LEN,
        seeds = [b"bridge_state"],
        bump,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    /// 初始管理员，同时作为账户创建的付款方。
    /// 必须匹配硬编码的 INITIAL_ADMIN 地址（在 initialize 指令中验证）。
    #[account(mut)]
    pub admin: Signer<'info>,
    /// CHECK: 金库 PDA，用作代币账户权限（authority）。
    /// 不持有数据，仅通过 seeds 派生地址。实际代币存储在关联的 TokenAccount 中。
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

// ─── 管理员操作（受时间锁保护） ──────────────────────────────────────────────

/// 管理员操作的通用账户上下文。
///
/// 适用于：configure、configure_rate_limits、
/// add/remove/rotate_relayer、propose_admin、set_guardian/operator/recovery。
///
/// 时间锁处理：timelock_op 是一个 UncheckedAccount，因为：
/// - 时间锁未激活时：客户端可传入 admin 自身账户（或任意账户），consume_timelock 直接放行
/// - 时间锁已激活时：必须传入匹配 op_hash 的 TimelockOperation PDA，在 consume_timelock 中验证
#[derive(Accounts)]
pub struct AdminOp<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    /// CHECK: 时间锁激活时为 TimelockOperation PDA；否则忽略。
    /// 在 consume_timelock 辅助函数中验证 PDA 地址、owner 和 eta 时间窗口。
    #[account(mut)]
    pub timelock_op: UncheckedAccount<'info>,
    #[account(mut)]
    pub admin: Signer<'info>,
}

// ─── Peer 链路管理 ───────────────────────────────────────────────────────────

/// 注册新 Peer 链路的账户上下文。
/// 创建以 chain_id 为种子的 PeerConfig PDA。
/// `init` 约束保证同一 chain_id 不能重复注册（PDA 已存在时创建失败）。
#[derive(Accounts)]
#[instruction(chain_id: u64)]
pub struct RegisterPeer<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        init,
        payer = admin,
        space = PeerConfig::LEN,
        seeds = [b"peer_config", chain_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub peer_config: Account<'info, PeerConfig>,
    /// CHECK: 时间锁激活时为 TimelockOperation PDA；否则忽略。
    #[account(mut)]
    pub timelock_op: UncheckedAccount<'info>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

/// Peer 链路管理操作的通用账户上下文。
/// 适用于：configure_peer、configure_peer_fee、configure_peer_rate_limits。
#[derive(Accounts)]
#[instruction(chain_id: u64)]
pub struct PeerAdminOp<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        mut,
        seeds = [b"peer_config", chain_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub peer_config: Account<'info, PeerConfig>,
    /// CHECK: 时间锁激活时为 TimelockOperation PDA；否则忽略。
    #[account(mut)]
    pub timelock_op: UncheckedAccount<'info>,
    #[account(mut)]
    pub admin: Signer<'info>,
}

/// 注销 Peer 链路的账户上下文。
/// 使用 `close = admin` 关闭 PeerConfig PDA 并退还租金。
#[derive(Accounts)]
#[instruction(chain_id: u64)]
pub struct UnregisterPeer<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        mut,
        close = admin,
        seeds = [b"peer_config", chain_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub peer_config: Account<'info, PeerConfig>,
    /// CHECK: 时间锁激活时为 TimelockOperation PDA；否则忽略。
    #[account(mut)]
    pub timelock_op: UncheckedAccount<'info>,
    #[account(mut)]
    pub admin: Signer<'info>,
}

// ─── 激活时间锁（无需时间锁 PDA） ───────────────────────────────────────────

/// 激活时间锁的账户上下文。
/// 与 AdminOp 的区别：不需要 timelock_op 账户（激活操作本身不受时间锁保护）。
#[derive(Accounts)]
pub struct ActivateTimelock<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    pub admin: Signer<'info>,
}

// ─── 调度 / 取消操作 ────────────────────────────────────────────────────────

/// 调度时间锁操作的账户上下文。
/// 创建一个以 sha256(data) 为种子的 TimelockOperation PDA，记录 eta 和操作哈希。
///
/// 指令参数仅 `data`，op_hash 由指令体内 `sha256(data)` 计算后用于 PDA 派生
/// 与 `system_program::create_account` CPI（不走 Anchor `init`/`seeds`，
/// 因 Anchor IDL 构建无法解析 seed 表达式中对 `Vec<u8>` 参数的函数调用）。
/// 与 EVM `scheduleOperation(bytes calldata data)` 1:1 对齐，结构上消除 InvalidEventData。
#[derive(Accounts)]
pub struct ScheduleOperation<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
        constraint = bridge_state.timelock_active @ ErrorCode::TimelockNotActive,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    /// CHECK: PDA 地址由 `find_program_address([b"timelock", op_hash], program_id)` 验证；
    /// 账户创建在指令体内通过 `create_account` CPI（PDA signer）完成。
    #[account(mut)]
    pub timelock_op: UncheckedAccount<'info>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

/// 取消已调度操作的账户上下文。
/// 使用 `close = admin` 关闭 TimelockOperation PDA 并退还租金。
/// 桥暂停时也可调用（无 is_paused 约束），允许在紧急冻结后清理待执行操作。
#[derive(Accounts)]
#[instruction(op_hash: [u8; 32])]
pub struct CancelOperation<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        mut,
        close = admin,
        seeds = [b"timelock", op_hash.as_ref()],
        bump,
    )]
    pub timelock_op: Account<'info, TimelockOperation>,
    #[account(mut)]
    pub admin: Signer<'info>,
}

// ─── 接受管理员 ──────────────────────────────────────────────────────────────

/// 两步管理员转移的第 2 步：新管理员接受转移。
/// 仅 pending_admin 可调用（通过 constraint 校验）。
#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
        constraint = new_admin.key() == bridge_state.pending_admin @ ErrorCode::Unauthorized,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    pub new_admin: Signer<'info>,
}

// ─── 监护人 / 恢复 ──────────────────────────────────────────────────────────

/// Guardian 紧急冻结的账户上下文。
/// 仅 guardian 可调用，仅在未暂停状态下可调用。
/// 冻结后 admin 无法解除，只有 recovery 可通过 ExecuteRecovery 恢复。
#[derive(Accounts)]
pub struct GuardianFreeze<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = guardian.key() == bridge_state.guardian @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    pub guardian: Signer<'info>,
}

/// Recovery 恢复操作的账户上下文。
/// 仅 recovery 可调用，仅在暂停状态下可调用。
/// 用于更换 admin（可选替换 guardian）并解除暂停。
#[derive(Accounts)]
pub struct ExecuteRecovery<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = recovery.key() == bridge_state.recovery @ ErrorCode::Unauthorized,
        constraint = bridge_state.is_paused @ ErrorCode::NotPaused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    pub recovery: Signer<'info>,
}

// ─── 质押 ────────────────────────────────────────────────────────────────────

/// 用户 stake USDC 的账户上下文（多 Peer 版本）。
///
/// nonce 由客户端生成随机值传入，用作 StakeRecord PDA 的种子。
/// PDA 的 `init` 约束天然防止 nonce 碰撞（PDA 已存在时创建失败）。
///
/// target_chain_id 用于派生 PeerConfig PDA，Anchor 的 seeds 约束保证只有已注册的 peer 才能被 stake。
#[derive(Accounts)]
#[instruction(nonce: u64, amount: u64, receiver: [u8; 32], _target_chain_id: u64)]
pub struct StakeAccounts<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
        constraint = bridge_state.usdc_mint != Pubkey::default() @ ErrorCode::UsdcNotConfigured,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    /// PeerConfig PDA，通过 target_chain_id 派生。PDA 存在即表示 peer 已注册。
    #[account(
        seeds = [b"peer_config", _target_chain_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub peer_config: Account<'info, PeerConfig>,
    /// 以 nonce 为种子创建的质押记录，记录 owner、amount 和 target_chain_id 用于退款
    #[account(
        init,
        payer = user,
        space = StakeRecord::LEN,
        seeds = [b"stake_record", nonce.to_le_bytes().as_ref()],
        bump,
    )]
    pub stake_record: Account<'info, StakeRecord>,
    #[account(mut)]
    pub user: Signer<'info>,
    /// CHECK: 金库 PDA，通过 seeds + 存储的 bump 验证。
    /// 使用存储的 bump 避免运行时 find_program_address 调用。
    #[account(seeds = [b"vault"], bump = bridge_state.vault_bump)]
    pub vault: AccountInfo<'info>,
    /// USDC 铸币账户，constraint 确保与 bridge_state 配置一致
    #[account(
        constraint = usdc_mint.key() == bridge_state.usdc_mint @ ErrorCode::UsdcNotConfigured
    )]
    pub usdc_mint: InterfaceAccount<'info, Mint>,
    /// 用户的 USDC 代币账户（转出方）
    #[account(
        mut,
        constraint = user_token_account.owner == user.key(),
        constraint = user_token_account.mint == usdc_mint.key(),
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,
    /// 金库的 USDC 代币账户（转入方）
    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key(),
        constraint = vault_token_account.mint == usdc_mint.key(),
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

// ─── 确认事件（中继器投票） ──────────────────────────────────────────────────

/// 中继器确认跨链事件的账户上下文（多 Peer 版本）。
///
/// CrossChainRequest 使用 init_if_needed：首个中继器创建 PDA 并支付租金，
/// 后续中继器复用已有 PDA 继续投票。达到阈值后自动触发解锁转账。
///
/// PDA seeds 加入 source_chain_id 隔离不同源链的 nonce 空间。
/// peer_config 通过 source_chain_id 派生，校验来源链路的合法性。
/// 指令参数仅 event_data 一个，nonce / source_chain_id 直接从 event_data 取，
/// 与 EVM `confirmEvent(BridgeEventData)` 1:1 对齐，结构上消除 NonceMismatch 与
/// SourceChainIdMismatch。
#[derive(Accounts)]
#[instruction(event_data: BridgeEventData)]
pub struct ConfirmEvent<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
        constraint = bridge_state.usdc_mint != Pubkey::default() @ ErrorCode::UsdcNotConfigured,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    /// PeerConfig PDA，通过 event_data.source_chain_id 派生。PDA 存在即表示来源链路已注册。
    #[account(
        mut,
        seeds = [b"peer_config", event_data.source_chain_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub peer_config: Account<'info, PeerConfig>,
    /// 跨链请求 PDA。init_if_needed 使首个中继器创建，后续复用。
    /// seeds 加入 event_data.source_chain_id 隔离不同源链的 nonce 空间。
    #[account(
        init_if_needed,
        payer = relayer,
        space = CrossChainRequest::LEN,
        seeds = [
            b"cross_chain_request",
            event_data.source_chain_id.to_le_bytes().as_ref(),
            event_data.nonce.to_le_bytes().as_ref(),
        ],
        bump,
    )]
    pub cross_chain_request: Account<'info, CrossChainRequest>,
    /// 中继器签名者。身份通过 bridge_state.is_relayer() 在指令逻辑中校验。
    #[account(mut)]
    pub relayer: Signer<'info>,
    /// CHECK: 金库 PDA，通过 seeds + 存储的 bump 验证
    #[account(seeds = [b"vault"], bump = bridge_state.vault_bump)]
    pub vault: AccountInfo<'info>,
    #[account(
        constraint = usdc_mint.key() == bridge_state.usdc_mint @ ErrorCode::UsdcNotConfigured
    )]
    pub usdc_mint: InterfaceAccount<'info, Mint>,
    /// 金库的 USDC 代币账户（解锁时的转出方）
    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key(),
        constraint = vault_token_account.mint == usdc_mint.key(),
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    /// 接收者的 USDC 代币账户（解锁时的转入方）。
    /// mint 在约束中校验；owner 与 event_data.receiver 的匹配在指令逻辑中
    /// 对所有投票统一验证（而非仅在解锁时），防止最终投票者传入错误账户导致 nonce 卡死。
    #[account(
        mut,
        constraint = receiver_token_account.mint == usdc_mint.key()
            @ ErrorCode::ReceiverMintMismatch,
    )]
    pub receiver_token_account: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

// ─── 操作员：跳过 Nonce ──────────────────────────────────────────────────────

/// 操作员跳过 nonce 的账户上下文（多 Peer 版本）。
/// 将 CrossChainRequest 标记为已处理，使该 nonce 永远无法被 unlock。
/// 配合发送端 initiate_refund + execute_refund 退还用户资金。
/// ⚠️ 必须在发送端退款之前调用，否则存在双花风险。
///
/// PDA seeds 加入 source_chain_id 匹配 confirm_event 的隔离策略。
///
/// 注意：此上下文不包含 PeerConfig 账户约束，因此 operator 可为已 unregister 的对端链
/// 清理遗留 nonce。Operator 承担 PDA 租金（compact 后退还）。
/// `source_chain_id == local_chain_id` 这个特殊情况在指令体内显式 revert
/// （`ErrorCode::InvalidChainId`），防止 operator 误操作或自费 DoS 本链 nonce 空间。
#[derive(Accounts)]
#[instruction(nonce: u64, source_chain_id: u64)]
pub struct SkipNonce<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = operator.key() == bridge_state.operator @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    /// 使用 init_if_needed：如果该 nonce 的 CrossChainRequest 不存在，
    /// 则创建一个并立即标记为已处理。
    #[account(
        init_if_needed,
        payer = operator,
        space = CrossChainRequest::LEN,
        seeds = [b"cross_chain_request", source_chain_id.to_le_bytes().as_ref(), nonce.to_le_bytes().as_ref()],
        bump,
    )]
    pub cross_chain_request: Account<'info, CrossChainRequest>,
    #[account(mut)]
    pub operator: Signer<'info>,
    pub system_program: Program<'info, System>,
}

// ─── 操作员：发起退款（两步退款第 1 步） ────────────────────────────────────

/// 发起退款的账户上下文（仅 operator 可调用）。
/// 记录发起时间戳，需等待 REFUND_DELAY 后才能执行第 2 步。
/// 无需代币账户，因为此步不执行转账。
#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct InitiateRefund<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = operator.key() == bridge_state.operator @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        mut,
        seeds = [b"stake_record", nonce.to_le_bytes().as_ref()],
        bump,
        constraint = stake_record.owner != Pubkey::default() @ ErrorCode::ZeroAddress,
        constraint = !stake_record.refunded @ ErrorCode::AlreadyRefunded,
        constraint = stake_record.refund_initiated_at == 0 @ ErrorCode::RefundAlreadyInitiated,
    )]
    pub stake_record: Account<'info, StakeRecord>,
    pub operator: Signer<'info>,
}

// ─── 执行退款（两步退款第 2 步） ────────────────────────────────────────────

/// 执行退款的账户上下文（operator 或原始 staker 均可调用）。
/// 需等待 REFUND_DELAY 后方可执行，受全局速率限制和金库最低储备约束。
/// ⚠️ 必须先在对端链 skip_nonce 封死 unlock，再发起退款，否则存在双花风险。
#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct ExecuteRefund<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        mut,
        seeds = [b"stake_record", nonce.to_le_bytes().as_ref()],
        bump,
        constraint = stake_record.owner != Pubkey::default() @ ErrorCode::ZeroAddress,
        constraint = !stake_record.refunded @ ErrorCode::AlreadyRefunded,
        constraint = stake_record.refund_initiated_at != 0 @ ErrorCode::RefundNotInitiated,
    )]
    pub stake_record: Account<'info, StakeRecord>,
    /// 调用者：必须是 operator 或原始 staker（stake_record.owner）
    #[account(
        constraint = caller.key() == bridge_state.operator || caller.key() == stake_record.owner @ ErrorCode::Unauthorized,
    )]
    pub caller: Signer<'info>,
    /// CHECK: 金库 PDA，通过 seeds + 存储的 bump 验证
    #[account(seeds = [b"vault"], bump = bridge_state.vault_bump)]
    pub vault: AccountInfo<'info>,
    #[account(
        constraint = usdc_mint.key() == bridge_state.usdc_mint @ ErrorCode::UsdcNotConfigured
    )]
    pub usdc_mint: InterfaceAccount<'info, Mint>,
    /// 金库的 USDC 代币账户（退款转出方）
    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key(),
        constraint = vault_token_account.mint == usdc_mint.key(),
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    /// 原始 staker 的 USDC 代币账户（退款转入方）。
    /// constraint 确保 owner 与 StakeRecord.owner 一致。
    #[account(
        mut,
        constraint = owner_token_account.owner == stake_record.owner @ ErrorCode::InvalidReceiver,
        constraint = owner_token_account.mint == usdc_mint.key(),
    )]
    pub owner_token_account: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
}

// ─── 管理员：取消退款 ────────────────────────────────────────────────────────

/// 取消已发起退款的账户上下文（仅 admin 可调用，暂停时也可调用）。
/// 用于 operator 密钥泄露后阻止恶意退款执行。
#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct CancelRefund<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        mut,
        seeds = [b"stake_record", nonce.to_le_bytes().as_ref()],
        bump,
        constraint = stake_record.refund_initiated_at != 0 @ ErrorCode::RefundNotInitiated,
    )]
    pub stake_record: Account<'info, StakeRecord>,
    pub admin: Signer<'info>,
}

// ─── 提取代币 ────────────────────────────────────────────────────────────────

/// 管理员从金库提取代币的账户上下文（受时间锁保护）。
/// 用于处理误转入的代币或按需转移资金。
///
/// 注意：`token_mint` 有意**不**约束为 `bridge_state.usdc_mint`。
/// 这是为了允许 admin 提取误转入金库的非 USDC 代币。
/// 安全性由时间锁保证：`op_hash` 中包含了 mint 地址和接收方地址，
/// 因此每种代币的提取都需要独立的 timelock 调度和审批。
#[derive(Accounts)]
#[instruction(amount: u64, to: Pubkey)]
pub struct WithdrawToken<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    /// CHECK: 时间锁激活时为 TimelockOperation PDA；否则忽略。
    /// 在 consume_timelock 辅助函数中验证。
    #[account(mut)]
    pub timelock_op: UncheckedAccount<'info>,
    #[account(mut)]
    pub admin: Signer<'info>,
    /// CHECK: 金库 PDA，通过 seeds + 存储的 bump 验证
    #[account(seeds = [b"vault"], bump = bridge_state.vault_bump)]
    pub vault: AccountInfo<'info>,
    /// 要提取的代币 mint。不限于 bridge USDC，允许提取任意误转入代币。
    pub token_mint: InterfaceAccount<'info, Mint>,
    /// 金库的代币账户（提取转出方）
    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key(),
        constraint = vault_token_account.mint == token_mint.key(),
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    /// 目标代币账户（提取转入方）。owner 必须匹配 `to` 参数，
    /// 确保 timelock op_hash 中承诺的接收方与实际转账目标一致。
    #[account(
        mut,
        constraint = to_token_account.mint == token_mint.key(),
        constraint = to_token_account.owner == to @ ErrorCode::InvalidReceiver,
    )]
    pub to_token_account: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
}
