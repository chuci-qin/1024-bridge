use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_error::ProgramError;

use crate::errors::ErrorCode;
use crate::events::OperationExecuted;
use crate::state::*;

// ─── 角色校验 ────────────────────────────────────────────────────────────────

pub fn check_roles_unique(
    admin: &Pubkey,
    guardian: &Pubkey,
    operator: &Pubkey,
    recovery: &Pubkey,
) -> Result<()> {
    require!(admin != guardian, ErrorCode::RoleOverlap);
    require!(admin != operator, ErrorCode::RoleOverlap);
    require!(admin != recovery, ErrorCode::RoleOverlap);
    require!(guardian != operator, ErrorCode::RoleOverlap);
    require!(guardian != recovery, ErrorCode::RoleOverlap);
    require!(operator != recovery, ErrorCode::RoleOverlap);
    Ok(())
}

// ─── 时间锁 ──────────────────────────────────────────────────────────────────

pub fn compute_op_hash(data: &[u8]) -> [u8; 32] {
    solana_sha256_hasher::hash(data).to_bytes()
}

/// 消费时间锁操作：验证 PDA、检查 eta + 宽限期、关闭账户。
/// 时间锁未激活时为空操作。
pub fn consume_timelock<'info>(
    bridge_state: &BridgeState,
    timelock_op_info: &AccountInfo<'info>,
    op_hash: &[u8; 32],
    admin_info: &AccountInfo<'info>,
    program_id: &Pubkey,
) -> Result<()> {
    if !bridge_state.timelock_active {
        return Ok(());
    }

    let (expected_pda, _) =
        Pubkey::find_program_address(&[b"timelock", op_hash.as_ref()], program_id);
    require!(
        timelock_op_info.key() == expected_pda,
        ErrorCode::TimelockNotScheduled
    );
    require!(
        *timelock_op_info.owner == *program_id,
        ErrorCode::TimelockNotScheduled
    );

    {
        let data = timelock_op_info.try_borrow_data()?;
        require!(
            data.len() >= TimelockOperation::LEN,
            ErrorCode::TimelockNotScheduled
        );
        let eta = u64::from_le_bytes(data[8..16].try_into().unwrap());

        let clock = Clock::get()?;
        let now = clock.unix_timestamp as u64;
        require!(now >= eta, ErrorCode::TimelockNotReady);
        require!(
            now <= eta.saturating_add(TIMELOCK_GRACE_PERIOD),
            ErrorCode::TimelockExpired
        );
    }

    let lamports = timelock_op_info.lamports();
    **timelock_op_info.try_borrow_mut_lamports()? = 0;
    **admin_info.try_borrow_mut_lamports()? = admin_info
        .lamports()
        .checked_add(lamports)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    timelock_op_info.try_borrow_mut_data()?.fill(0);

    emit!(OperationExecuted { op_hash: *op_hash });

    Ok(())
}

// ─── 转账限制 ────────────────────────────────────────────────────────────────

/// 统一转出检查：速率限制 + 单笔限额。
pub fn check_transfer_limits(bridge_state: &mut BridgeState, amount: u64) -> Result<()> {
    check_rate_limit(bridge_state, amount)?;
    if bridge_state.max_single_unlock != 0 && amount > bridge_state.max_single_unlock {
        return err!(ErrorCode::SingleTransferExceeded);
    }
    Ok(())
}

/// 滑动窗口速率限制（对应 EVM 的 _checkRateLimit）。
fn check_rate_limit(bs: &mut BridgeState, amount: u64) -> Result<()> {
    let max_per_window = bs.max_unlock_per_window;
    let duration = bs.window_duration;
    if max_per_window == 0 || duration == 0 {
        return Ok(());
    }

    let clock = Clock::get()?;
    let now = clock.unix_timestamp as u64;
    let window_start = bs.current_window_start;

    if now >= window_start.saturating_add(duration) {
        if now < window_start.saturating_add(duration.saturating_mul(2)) {
            bs.previous_window_usage = bs.current_window_usage;
        } else {
            bs.previous_window_usage = 0;
        }
        bs.current_window_usage = 0;
        bs.current_window_start = now;
    }

    let elapsed = now.saturating_sub(bs.current_window_start);
    let remaining_weight = duration.saturating_sub(elapsed);
    let sliding_usage = (bs.previous_window_usage as u128)
        .checked_mul(remaining_weight as u128)
        .unwrap_or(0)
        / duration as u128
        + bs.current_window_usage as u128;

    if sliding_usage + amount as u128 > max_per_window as u128 {
        return err!(ErrorCode::RateLimitExceeded);
    }

    bs.current_window_usage = bs
        .current_window_usage
        .checked_add(amount)
        .ok_or_else(|| error!(ErrorCode::RateLimitExceeded))?;

    Ok(())
}

/// 金库储备不变式：余额必须覆盖解锁金额 + 最低储备。
pub fn check_vault_invariant(
    vault_balance: u64,
    unlock_amount: u64,
    minimum_reserve: u64,
) -> Result<()> {
    let required = (unlock_amount as u128) + (minimum_reserve as u128);
    if (vault_balance as u128) < required {
        return err!(ErrorCode::InsufficientReserve);
    }
    Ok(())
}
