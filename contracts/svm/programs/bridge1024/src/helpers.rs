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

/// 通用滑动窗口速率限制检查。
///
/// 供 BridgeState 全局限制和 PeerConfig per-chain 限制复用。
/// 通过可变引用接收窗口状态字段，更新窗口位置和使用量。
///
/// 相比固定窗口，滑动窗口通过加权上一窗口的剩余时间占比来平滑流量，
/// 避免攻击者在两个固定窗口的交界处集中发起大量解锁。
///
/// 使用 u128 中间值避免乘法溢出。
pub fn check_sliding_window_rate_limit(
    max_per_window: u64,
    window_duration: u64,
    current_window_start: &mut u64,
    current_window_usage: &mut u64,
    previous_window_usage: &mut u64,
    amount: u64,
) -> Result<()> {
    if max_per_window == 0 || window_duration == 0 {
        return Ok(());
    }

    let clock = Clock::get()?;
    let now = clock.unix_timestamp as u64;

    if now >= current_window_start.saturating_add(window_duration) {
        if now < current_window_start.saturating_add(window_duration.saturating_mul(2)) {
            *previous_window_usage = *current_window_usage;
        } else {
            *previous_window_usage = 0;
        }
        *current_window_usage = 0;
        *current_window_start = now;
    }

    let elapsed = now.saturating_sub(*current_window_start);
    let remaining_weight = window_duration.saturating_sub(elapsed);
    let sliding_usage = (*previous_window_usage as u128)
        .saturating_mul(remaining_weight as u128)
        / window_duration as u128
        + *current_window_usage as u128;

    if sliding_usage + amount as u128 > max_per_window as u128 {
        return err!(ErrorCode::RateLimitExceeded);
    }

    *current_window_usage = current_window_usage
        .checked_add(amount)
        .ok_or_else(|| error!(ErrorCode::RateLimitExceeded))?;

    Ok(())
}

/// 双层转出限额检查：per-chain 速率限制 + 全局速率限制。
/// 在 unlock 路径（confirm_event）上调用，先检查 peer-chain 层，再检查全局层。
///
/// 此处不校验 max_single_unlock：confirm_event 入口已经对 per-chain 与全局限额做了早拒，
/// 到这里时必然已满足；在内层重复检查会成为事实上不可达的死代码且阅读上容易误导。
pub fn check_dual_transfer_limits(
    bridge_state: &mut BridgeState,
    peer_config: &mut PeerConfig,
    amount: u64,
) -> Result<()> {
    // per-chain 速率限制
    check_sliding_window_rate_limit(
        peer_config.max_unlock_per_window,
        peer_config.window_duration,
        &mut peer_config.current_window_start,
        &mut peer_config.current_window_usage,
        &mut peer_config.previous_window_usage,
        amount,
    )?;

    // 全局速率限制
    check_sliding_window_rate_limit(
        bridge_state.max_unlock_per_window,
        bridge_state.window_duration,
        &mut bridge_state.current_window_start,
        &mut bridge_state.current_window_usage,
        &mut bridge_state.previous_window_usage,
        amount,
    )?;

    Ok(())
}

/// 仅全局速率限制检查。在 execute_refund 路径上调用（退款不涉及 peer 链路出金）。
///
/// 此处不校验 max_single_unlock：退款是把用户已 stake 的资金原路退回，
/// 不变量"能 stake 就能 refund"要求只要 stake 通过了 peer_config.max_stake_amount，
/// 对应 refund 就必须可执行，否则事后调小该限额会使历史 stake 的退款被永久卡住。
/// 全局滑动窗口速率限制保留，作为退款大额连续出金的最终防线。
pub fn check_global_transfer_limits(bridge_state: &mut BridgeState, amount: u64) -> Result<()> {
    check_sliding_window_rate_limit(
        bridge_state.max_unlock_per_window,
        bridge_state.window_duration,
        &mut bridge_state.current_window_start,
        &mut bridge_state.current_window_usage,
        &mut bridge_state.previous_window_usage,
        amount,
    )?;

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
