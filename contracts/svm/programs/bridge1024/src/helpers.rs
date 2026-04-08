use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_error::ProgramError;

use crate::errors::ErrorCode;
use crate::events::OperationExecuted;
use crate::state::*;

// ─── 角色校验 ────────────────────────────────────────────────────────────────

/// 校验四角色（admin / guardian / operator / recovery）地址互不相同。
/// 角色分离是安全模型的基础：任意单一角色泄露不会导致资金丢失。
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

/// 计算单个连续字节切片的 SHA-256 哈希。
/// 用于 schedule_operation 中验证 op_hash == SHA-256(data)。
pub fn compute_op_hash(data: &[u8]) -> [u8; 32] {
    solana_sha256_hasher::hash(data).to_bytes()
}

/// 计算多个字节切片拼接后的 SHA-256 哈希（零堆分配）。
/// 用于管理指令中构造 op_hash，避免在 BPF 堆上分配 Vec。
/// 等价于 `hash(data[0] || data[1] || ...)`，因为 SHA-256 顺序处理字节流。
pub fn compute_op_hashv(data: &[&[u8]]) -> [u8; 32] {
    solana_sha256_hasher::hashv(data).to_bytes()
}

/// 消费（验证并销毁）一个时间锁操作 PDA。
///
/// 流程：
/// 1. 如果 timelock 未激活，直接放行（初始部署阶段）
/// 2. 验证 timelock_op PDA 地址和 owner 匹配
/// 3. 反序列化 TimelockOperation，检查 eta ≤ now ≤ eta + GRACE_PERIOD
/// 4. 关闭 PDA 账户：清零数据、转移 lamports 给 admin
///
/// 手动关闭账户而非使用 Anchor 的 `close` 约束，因为 AdminOp 使用
/// UncheckedAccount（timelock 未激活时可传入任意账户）。
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

    // 验证 PDA 地址：seeds = [b"timelock", op_hash]
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

    // 反序列化并校验时间窗口
    {
        let data = timelock_op_info.try_borrow_data()?;
        let tl = TimelockOperation::try_deserialize(&mut &data[..])
            .map_err(|_| error!(ErrorCode::TimelockNotScheduled))?;

        let clock = Clock::get()?;
        let now = clock.unix_timestamp as u64;
        require!(now >= tl.eta, ErrorCode::TimelockNotReady);
        require!(
            now <= tl.eta.saturating_add(TIMELOCK_GRACE_PERIOD),
            ErrorCode::TimelockExpired
        );
    }

    // 手动关闭 PDA 账户（复制 Anchor close 行为）：
    // 1. 转移全部 lamports 给 admin
    // 2. 清零数据防止 discriminator 残留
    // 3. 将 owner 重置为 system_program，彻底防止同交易内账户复活
    let lamports = timelock_op_info.lamports();
    **timelock_op_info.try_borrow_mut_lamports()? = 0;
    **admin_info.try_borrow_mut_lamports()? = admin_info
        .lamports()
        .checked_add(lamports)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    timelock_op_info.try_borrow_mut_data()?.fill(0);
    timelock_op_info.assign(&anchor_lang::system_program::ID);

    emit!(OperationExecuted { op_hash: *op_hash });

    Ok(())
}

// ─── 转账限制 ────────────────────────────────────────────────────────────────

/// 统一的转出限额检查：速率限制 + 单笔限额。
/// 在 unlock、execute_refund 等所有出金路径上调用，作为统一安全关卡。
pub fn check_transfer_limits(bridge_state: &mut BridgeState, amount: u64) -> Result<()> {
    check_rate_limit(bridge_state, amount)?;
    if bridge_state.max_single_unlock != 0 && amount > bridge_state.max_single_unlock {
        return err!(ErrorCode::SingleTransferExceeded);
    }
    Ok(())
}

/// 滑动窗口速率限制（对应 EVM 的 _checkRateLimit）。
///
/// 相比固定窗口，滑动窗口通过加权上一窗口的剩余时间占比来平滑流量，
/// 避免攻击者在两个固定窗口的交界处集中发起大量解锁。
///
/// 算法：
/// - 如果当前时间超出窗口，滚动窗口（保留上一窗口用量或清零）
/// - 计算滑动使用量 = previous_usage × (remaining_time / duration) + current_usage
/// - 如果 sliding_usage + amount > max_per_window，则拒绝
///
/// 使用 u128 中间值避免乘法溢出。
fn check_rate_limit(bs: &mut BridgeState, amount: u64) -> Result<()> {
    let max_per_window = bs.max_unlock_per_window;
    let duration = bs.window_duration;
    if max_per_window == 0 || duration == 0 {
        return Ok(());
    }

    let clock = Clock::get()?;
    let now = clock.unix_timestamp as u64;
    let window_start = bs.current_window_start;

    // 窗口滚动：判断当前时间是否超出当前窗口
    if now >= window_start.saturating_add(duration) {
        if now < window_start.saturating_add(duration.saturating_mul(2)) {
            // 在相邻的下一个窗口内：保留当前用量作为"上一窗口用量"
            bs.previous_window_usage = bs.current_window_usage;
        } else {
            // 跨越了两个以上窗口：上一窗口用量已无参考价值，清零
            bs.previous_window_usage = 0;
        }
        bs.current_window_usage = 0;
        bs.current_window_start = now;
    }

    // 计算滑动窗口加权使用量
    let elapsed = now.saturating_sub(bs.current_window_start);
    let remaining_weight = duration.saturating_sub(elapsed);
    let sliding_usage = (bs.previous_window_usage as u128)
        .saturating_mul(remaining_weight as u128)
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

/// 金库储备不变式检查：确保转出后金库余额不低于最低储备金要求。
/// 这是最后一道安全防线，防止金库被完全掏空。
/// 使用 u128 避免加法溢出。
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

// ─── PDA 压缩 ────────────────────────────────────────────────────────────────

/// 压缩已完成的 CrossChainRequest PDA：缩小账户至最小尺寸，退还多余租金。
///
/// 在 unlock 达到阈值或 skip_nonce 标记完成后立即调用，无需额外交易。
/// PDA 保留 is_unlocked / is_skipped 标志永久阻止 nonce 重用（防止 init_if_needed 双花），
/// 但清空投票数据以释放 ~88% 的租金给调用者。
pub fn compact_request_pda<'info>(
    req_info: &AccountInfo<'info>,
    refund_to: &AccountInfo<'info>,
) -> Result<()> {
    let old_lamports = req_info.lamports();
    req_info.resize(CrossChainRequest::COMPACTED_LEN)?;

    let rent = Rent::get()?;
    let new_minimum = rent.minimum_balance(CrossChainRequest::COMPACTED_LEN);
    let refund = old_lamports.saturating_sub(new_minimum);
    if refund > 0 {
        **req_info.try_borrow_mut_lamports()? -= refund;
        **refund_to.try_borrow_mut_lamports()? = refund_to
            .lamports()
            .checked_add(refund)
            .ok_or(ProgramError::ArithmeticOverflow)?;
    }
    Ok(())
}
