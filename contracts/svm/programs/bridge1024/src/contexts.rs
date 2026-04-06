use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::errors::ErrorCode;
use crate::state::*;

// ─── 初始化 ──────────────────────────────────────────────────────────────────

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
    /// CHECK: 金库 PDA，用作代币账户权限，通过 seeds 验证
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

// ─── 管理员操作（受时间锁保护） ──────────────────────────────────────────────

/// 管理员操作的通用上下文，可能需要时间锁。
/// 时间锁未激活时，将 admin 自身账户传入 timelock_op 即可。
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
    /// 在 consume_timelock 辅助函数中验证。
    #[account(mut)]
    pub timelock_op: UncheckedAccount<'info>,
    #[account(mut)]
    pub admin: Signer<'info>,
}

// ─── 激活时间锁（无需时间锁 PDA） ───────────────────────────────────────────

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

#[derive(Accounts)]
#[instruction(op_hash: [u8; 32])]
pub struct ScheduleOperation<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
        constraint = bridge_state.timelock_active @ ErrorCode::TimelockNotActive,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        init,
        payer = admin,
        space = TimelockOperation::LEN,
        seeds = [b"timelock", op_hash.as_ref()],
        bump,
    )]
    pub timelock_op: Account<'info, TimelockOperation>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

/// cancel_operation 在桥暂停时也可调用（无 is_paused 约束）。
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

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct StakeAccounts<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
        constraint = bridge_state.usdc_mint != Pubkey::default() @ ErrorCode::UsdcNotConfigured,
        constraint = nonce == bridge_state.sender_nonce @ ErrorCode::NonceMismatch,
    )]
    pub bridge_state: Account<'info, BridgeState>,
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
    /// CHECK: 金库 PDA，通过 seeds 验证
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    #[account(
        constraint = usdc_mint.key() == bridge_state.usdc_mint @ ErrorCode::UsdcNotConfigured
    )]
    pub usdc_mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        constraint = user_token_account.owner == user.key(),
        constraint = user_token_account.mint == usdc_mint.key(),
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,
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

#[derive(Accounts)]
#[instruction(_nonce: u64)]
pub struct ConfirmEvent<'info> {
    #[account(
        mut,
        seeds = [b"bridge_state"],
        bump,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
        constraint = bridge_state.usdc_mint != Pubkey::default() @ ErrorCode::UsdcNotConfigured,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        init_if_needed,
        payer = relayer,
        space = CrossChainRequest::LEN,
        seeds = [b"cross_chain_request", _nonce.to_le_bytes().as_ref()],
        bump,
    )]
    pub cross_chain_request: Account<'info, CrossChainRequest>,
    #[account(mut)]
    pub relayer: Signer<'info>,
    /// CHECK: 金库 PDA，通过 seeds 验证
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    #[account(
        constraint = usdc_mint.key() == bridge_state.usdc_mint @ ErrorCode::UsdcNotConfigured
    )]
    pub usdc_mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key(),
        constraint = vault_token_account.mint == usdc_mint.key(),
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
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

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct SkipNonce<'info> {
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = operator.key() == bridge_state.operator @ ErrorCode::Unauthorized,
        constraint = !bridge_state.is_paused @ ErrorCode::Paused,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(
        init_if_needed,
        payer = operator,
        space = CrossChainRequest::LEN,
        seeds = [b"cross_chain_request", nonce.to_le_bytes().as_ref()],
        bump,
    )]
    pub cross_chain_request: Account<'info, CrossChainRequest>,
    #[account(mut)]
    pub operator: Signer<'info>,
    pub system_program: Program<'info, System>,
}

// ─── 操作员：退款 ────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct RefundAccounts<'info> {
    #[account(
        mut,
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
        constraint = stake_record.amount > 0 @ ErrorCode::ZeroAmount,
        constraint = !stake_record.refunded @ ErrorCode::AlreadyRefunded,
    )]
    pub stake_record: Account<'info, StakeRecord>,
    pub operator: Signer<'info>,
    /// CHECK: 金库 PDA，通过 seeds 验证
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    #[account(
        constraint = usdc_mint.key() == bridge_state.usdc_mint @ ErrorCode::UsdcNotConfigured
    )]
    pub usdc_mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key(),
        constraint = vault_token_account.mint == usdc_mint.key(),
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        constraint = owner_token_account.owner == stake_record.owner @ ErrorCode::InvalidReceiver,
        constraint = owner_token_account.mint == usdc_mint.key(),
    )]
    pub owner_token_account: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
}

// ─── 提取代币 ────────────────────────────────────────────────────────────────

#[derive(Accounts)]
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
    #[account(mut)]
    pub timelock_op: UncheckedAccount<'info>,
    #[account(mut)]
    pub admin: Signer<'info>,
    /// CHECK: 金库 PDA，通过 seeds 验证
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    pub usdc_mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key(),
        constraint = vault_token_account.mint == usdc_mint.key(),
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        constraint = to_token_account.mint == usdc_mint.key(),
    )]
    pub to_token_account: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
}

// ─── 关闭请求 ────────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct CloseRequest<'info> {
    #[account(
        mut,
        close = admin,
        seeds = [b"cross_chain_request", nonce.to_le_bytes().as_ref()],
        bump,
    )]
    pub cross_chain_request: Account<'info, CrossChainRequest>,
    #[account(
        seeds = [b"bridge_state"],
        bump,
        constraint = admin.key() == bridge_state.admin @ ErrorCode::Unauthorized,
    )]
    pub bridge_state: Account<'info, BridgeState>,
    #[account(mut)]
    pub admin: Signer<'info>,
}
