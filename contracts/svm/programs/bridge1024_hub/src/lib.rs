use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TransferChecked};

mod contexts;
mod errors;
mod events;
mod helpers;
mod state;

use contexts::*;
use errors::ErrorCode;
use events::*;
use helpers::*;
use state::*;

declare_id!("2DzrZV6EYPbRa1KQSZhLQSQ3SENJApjrLgBEzb65Wtou");

/// 硬编码的初始管理员地址（2XVdXwC235qFXSm5egXpWyNY9xaiShFD5HKGrEhQNEFY）。
/// 部署前必须设置为实际部署者的公钥。
/// 防止 initialize 被抢先调用（front-running），比 verify_upgrade_authority 更可靠，
/// 无 Solana 版本兼容性问题。
pub const INITIAL_ADMIN: Pubkey = Pubkey::new_from_array([
    22, 171, 123, 173, 77, 255, 198, 3, 77, 94, 188, 132, 148, 188, 245, 57,
    58, 135, 108, 181, 100, 2, 76, 171, 21, 38, 157, 187, 65, 193, 151, 151,
]);

/// Bridge1024 SVM 跨链桥程序（多 Peer 版本）。
///
/// 本程序是 Bridge1024 跨链桥的 Solana 端实现，支持 stake（锁定）和 unlock（解锁）两种核心操作。
/// 多 Peer 版本通过独立的 PeerConfig PDA 管理每条链路的配置，支持同时连接多条对端链。
///
/// 核心流程：
/// - 出金：用户 stake → 中继器监听 Staked → 在对端链 confirm_event → 达到阈值自动 unlock
/// - 异常：operator skip_nonce（接收端）→ operator initiate_refund → execute_refund（发送端）
///
/// 安全机制：
/// - 四角色分离（admin / guardian / operator / recovery）
/// - 时间锁（24h 延迟 + 48h 执行窗口）
/// - 双层滑动窗口速率限制（per-chain + 全局）
/// - 金库最低储备金
/// - 白名单中继器哈希投票（2/3 阈值）
/// - 紧急冻结与恢复
///
/// SVM 特有功能：
/// - 源链手续费（stake 时扣除 bridge_fee，per-chain 配置；unlock 全额转给用户）
/// - vault_bump 缓存（避免重复 find_program_address）
/// - Token-2022（token_interface）兼容
#[program]
pub mod bridge1024_hub {
    use super::*;

    // ─── 初始化 ──────────────────────────────────────────────────────────

    /// 创建 BridgeState PDA，设置四角色分离。
    ///
    /// 所有角色地址必须非零且互不相同。
    /// 安全性：通过硬编码 INITIAL_ADMIN 地址防止 front-running。
    /// Anchor 的 `init` 约束会零初始化账户，因此只需设置非零字段。
    /// vault_bump 在此处缓存，后续 CPI 调用使用存储值避免重复 PDA 查找。
    pub fn initialize(
        ctx: Context<Initialize>,
        guardian: Pubkey,
        operator: Pubkey,
        recovery: Pubkey,
    ) -> Result<()> {
        let admin = ctx.accounts.admin.key();
        require!(admin == INITIAL_ADMIN, ErrorCode::Unauthorized);
        require!(admin != Pubkey::default(), ErrorCode::ZeroAddress);
        require!(guardian != Pubkey::default(), ErrorCode::ZeroAddress);
        require!(operator != Pubkey::default(), ErrorCode::ZeroAddress);
        require!(recovery != Pubkey::default(), ErrorCode::ZeroAddress);
        check_roles_unique(&admin, &guardian, &operator, &recovery)?;

        let bs = &mut ctx.accounts.bridge_state;
        bs.admin = admin;
        bs.guardian = guardian;
        bs.operator = operator;
        bs.recovery = recovery;
        bs.vault_bump = ctx.bumps.vault;

        Ok(())
    }

    // ─── 时间锁 ──────────────────────────────────────────────────────────

    /// 不可逆地激活时间锁。
    ///
    /// 激活后所有关键管理操作需要：调度 → 等待 24 小时 → 在 48 小时窗口内执行。
    /// 初始部署阶段不激活，允许管理员快速完成首次配置；一经激活不可撤销。
    pub fn activate_timelock(ctx: Context<ActivateTimelock>) -> Result<()> {
        let bs = &mut ctx.accounts.bridge_state;
        require!(!bs.timelock_active, ErrorCode::TimelockAlreadyActive);
        bs.timelock_active = true;
        emit!(TimelockActivated {});
        Ok(())
    }

    /// 调度一个时间锁操作。
    ///
    /// 创建以 `op_hash` 为种子的 TimelockOperation PDA，记录 eta = now + 24h。
    /// `data` 为原始操作负载（如 `"configure" || usdc_mint || ...`）；
    /// `op_hash` 必须等于 SHA-256(data)，防止调度与执行时的参数不一致。
    pub fn schedule_operation(
        ctx: Context<ScheduleOperation>,
        op_hash: [u8; 32],
        data: Vec<u8>,
    ) -> Result<()> {
        require!(compute_op_hash(&data) == op_hash, ErrorCode::InvalidEventData);

        let clock = Clock::get()?;
        let eta = (clock.unix_timestamp as u64)
            .checked_add(TIMELOCK_DELAY)
            .ok_or_else(|| error!(ErrorCode::TimelockNotReady))?;

        let tl = &mut ctx.accounts.timelock_op;
        tl.eta = eta;
        tl.op_hash = op_hash;

        emit!(OperationScheduled { op_hash, eta, data });
        Ok(())
    }

    /// 取消已调度的操作。桥暂停时也可调用（用于紧急清理）。
    /// Anchor 的 `close` 约束会关闭 PDA 并退还租金给 admin。
    pub fn cancel_operation(
        ctx: Context<CancelOperation>,
        op_hash: [u8; 32],
    ) -> Result<()> {
        let _ = &ctx.accounts.timelock_op;
        emit!(OperationCancelled { op_hash });
        Ok(())
    }

    // ─── 管理员：全局配置 ────────────────────────────────────────────────

    /// 设置 USDC 铸币地址和本链 ID。
    ///
    /// 多 Peer 版本不再设置 peer_contract 和 peer_chain_id，这些移至 PeerConfig。
    /// 这些参数在部署后通常只设置一次。
    pub fn configure(
        ctx: Context<AdminOp>,
        usdc_mint: Pubkey,
        local_chain_id: u64,
    ) -> Result<()> {
        require!(usdc_mint != Pubkey::default(), ErrorCode::ZeroAddress);
        require!(local_chain_id != 0, ErrorCode::InvalidChainId);

        let op_hash = compute_op_hashv(&[
            b"configure",
            &usdc_mint.to_bytes(),
            &local_chain_id.to_le_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        bs.usdc_mint = usdc_mint;
        bs.local_chain_id = local_chain_id;

        emit!(BridgeConfigured {
            usdc_mint,
            local_chain_id,
        });
        Ok(())
    }

    /// 原子性设置全局速率限制参数，同时重置滑动窗口。
    ///
    /// 多 Peer 版本不再包含 max_stake（移至 per-peer configure_peer_rate_limits）。
    ///
    /// 参数约束：
    /// - max_per_window 与 window_duration 必须同时为零（禁用）或同时非零（启用）
    /// - max_single 不得超过 max_per_window
    /// - window_duration 至少 60 秒
    pub fn configure_rate_limits(
        ctx: Context<AdminOp>,
        max_per_window: u64,
        window_duration: u64,
        max_single: u64,
        min_reserve: u64,
    ) -> Result<()> {
        require!(
            (max_per_window == 0) == (window_duration == 0),
            ErrorCode::InvalidRateLimitParams
        );
        if max_per_window != 0 && max_single != 0 {
            require!(
                max_single <= max_per_window,
                ErrorCode::InvalidRateLimitParams
            );
        }
        if max_per_window != 0 && window_duration != 0 {
            require!(window_duration >= 60, ErrorCode::InvalidRateLimitParams);
        }

        let op_hash = compute_op_hashv(&[
            b"configureRateLimits",
            &max_per_window.to_le_bytes(),
            &window_duration.to_le_bytes(),
            &max_single.to_le_bytes(),
            &min_reserve.to_le_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        bs.max_unlock_per_window = max_per_window;
        bs.window_duration = window_duration;
        bs.max_single_unlock = max_single;
        bs.minimum_reserve = min_reserve;
        let clock = Clock::get()?;
        bs.current_window_start = clock.unix_timestamp as u64;
        bs.current_window_usage = 0;
        bs.previous_window_usage = 0;

        emit!(RateLimitsConfigured {
            max_unlock_per_window: max_per_window,
            window_duration,
            max_single_unlock: max_single,
            minimum_reserve: min_reserve,
        });
        Ok(())
    }

    // ─── 管理员：Peer 链路管理 ───────────────────────────────────────────

    /// 注册一个新的 Peer 链路，创建 PeerConfig PDA。
    ///
    /// chain_id 不得为 0 或等于 local_chain_id（自环）。
    /// bridge_fee 不得超过 MAX_FEE。
    /// per-chain 速率限制字段初始化为 0（不限制），可后续通过 configure_peer_rate_limits 设置。
    pub fn register_peer(
        ctx: Context<RegisterPeer>,
        chain_id: u64,
        peer_contract: [u8; 32],
        bridge_fee: u64,
        max_stake_amount: u64,
    ) -> Result<()> {
        require!(chain_id != 0, ErrorCode::InvalidChainId);
        require!(
            chain_id != ctx.accounts.bridge_state.local_chain_id,
            ErrorCode::InvalidLocalChainId
        );
        require!(peer_contract != [0u8; 32], ErrorCode::ZeroAddress);
        require!(bridge_fee <= MAX_FEE, ErrorCode::FeeTooHigh);

        let op_hash = compute_op_hashv(&[
            b"registerPeer",
            &chain_id.to_le_bytes(),
            &peer_contract,
            &bridge_fee.to_le_bytes(),
            &max_stake_amount.to_le_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let pc = &mut ctx.accounts.peer_config;
        pc.chain_id = chain_id;
        pc.peer_contract = peer_contract;
        pc.bridge_fee = bridge_fee;
        pc.max_stake_amount = max_stake_amount;

        emit!(PeerRegistered {
            chain_id,
            peer_contract,
            bridge_fee,
        });
        Ok(())
    }

    /// 更新已有 Peer 的合约地址。
    ///
    /// ⚠️ 修改 peer_contract 会导致所有进行中的 CrossChainRequest 因校验不匹配而永久卡住，
    /// 受影响的 nonce 需通过 skip_nonce + initiate_refund/execute_refund 流程处理退款。
    pub fn configure_peer(
        ctx: Context<PeerAdminOp>,
        chain_id: u64,
        peer_contract: [u8; 32],
    ) -> Result<()> {
        require!(peer_contract != [0u8; 32], ErrorCode::ZeroAddress);

        let op_hash = compute_op_hashv(&[
            b"configurePeer",
            &chain_id.to_le_bytes(),
            &peer_contract,
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        ctx.accounts.peer_config.peer_contract = peer_contract;

        emit!(PeerConfigured {
            chain_id,
            peer_contract,
        });
        Ok(())
    }

    /// 更新某条链路的手续费。
    ///
    /// fee 不得超过 MAX_FEE（1000 USDC），防止管理员误操作。
    pub fn configure_peer_fee(
        ctx: Context<PeerAdminOp>,
        chain_id: u64,
        fee: u64,
    ) -> Result<()> {
        require!(fee <= MAX_FEE, ErrorCode::FeeTooHigh);

        let op_hash = compute_op_hashv(&[
            b"configurePeerFee",
            &chain_id.to_le_bytes(),
            &fee.to_le_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        ctx.accounts.peer_config.bridge_fee = fee;

        emit!(PeerFeeConfigured { chain_id, fee });
        Ok(())
    }

    /// 设置某条链路的 per-chain 速率限制。
    ///
    /// 与全局 configure_rate_limits 相同的参数校验规则。
    /// 重置该链路的滑动窗口。
    pub fn configure_peer_rate_limits(
        ctx: Context<PeerAdminOp>,
        chain_id: u64,
        max_per_window: u64,
        window_duration: u64,
        max_single: u64,
        max_stake: u64,
    ) -> Result<()> {
        require!(
            (max_per_window == 0) == (window_duration == 0),
            ErrorCode::InvalidRateLimitParams
        );
        if max_per_window != 0 && max_single != 0 {
            require!(
                max_single <= max_per_window,
                ErrorCode::InvalidRateLimitParams
            );
        }
        if max_per_window != 0 && window_duration != 0 {
            require!(window_duration >= 60, ErrorCode::InvalidRateLimitParams);
        }

        let op_hash = compute_op_hashv(&[
            b"configurePeerRateLimits",
            &chain_id.to_le_bytes(),
            &max_per_window.to_le_bytes(),
            &window_duration.to_le_bytes(),
            &max_single.to_le_bytes(),
            &max_stake.to_le_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let pc = &mut ctx.accounts.peer_config;
        pc.max_unlock_per_window = max_per_window;
        pc.window_duration = window_duration;
        pc.max_single_unlock = max_single;
        pc.max_stake_amount = max_stake;
        let clock = Clock::get()?;
        pc.current_window_start = clock.unix_timestamp as u64;
        pc.current_window_usage = 0;
        pc.previous_window_usage = 0;

        emit!(PeerRateLimitsConfigured {
            chain_id,
            max_unlock_per_window: max_per_window,
            window_duration,
            max_single_unlock: max_single,
            max_stake_amount: max_stake,
        });
        Ok(())
    }

    /// 注销 Peer 链路，关闭 PeerConfig PDA 并退还租金。
    ///
    /// 注销后该链路的 stake 和 confirm_event 将因 PDA 不存在而自动 revert。
    /// 如需重新启用，调用 register_peer 重新注册。
    pub fn unregister_peer(
        ctx: Context<UnregisterPeer>,
        chain_id: u64,
    ) -> Result<()> {
        let op_hash = compute_op_hashv(&[
            b"unregisterPeer",
            &chain_id.to_le_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        emit!(PeerUnregistered { chain_id });
        Ok(())
    }

    // ─── 管理员：中继器管理 ──────────────────────────────────────────────

    /// 添加新的中继器到白名单。
    ///
    /// 遍历检查防止重复添加，总数不得超过 MAX_RELAYERS（18）。
    /// 受时间锁保护，防止恶意快速添加不受信任的中继器。
    pub fn add_relayer(ctx: Context<AdminOp>, relayer: Pubkey) -> Result<()> {
        require!(relayer != Pubkey::default(), ErrorCode::ZeroAddress);

        let op_hash = compute_op_hashv(&[
            b"addRelayer",
            &relayer.to_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        require!(
            bs.relayers.len() < MAX_RELAYERS,
            ErrorCode::TooManyRelayers
        );
        require!(
            !bs.relayers.contains(&relayer),
            ErrorCode::RelayerAlreadyExists
        );

        bs.relayers.push(relayer);
        emit!(RelayerAdded { relayer });
        Ok(())
    }

    /// 从白名单移除中继器。
    ///
    /// 使用 swap_remove（交换到末尾再 pop）以节省 CU，不保证数组顺序。
    pub fn remove_relayer(ctx: Context<AdminOp>, relayer: Pubkey) -> Result<()> {
        let op_hash = compute_op_hashv(&[
            b"removeRelayer",
            &relayer.to_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        let idx = bs
            .relayers
            .iter()
            .position(|r| *r == relayer)
            .ok_or(error!(ErrorCode::RelayerNotFound))?;
        bs.relayers.swap_remove(idx);

        emit!(RelayerRemoved { relayer });
        Ok(())
    }

    /// 原子化替换一个中继器：移除旧的、添加新的，无需两步操作。
    ///
    /// 遍历时同时检查旧地址是否存在和新地址是否冲突。
    /// 单次时间锁调度即可完成替换，比分开 remove + add 更高效且原子。
    pub fn rotate_relayer(
        ctx: Context<AdminOp>,
        old_relayer: Pubkey,
        new_relayer: Pubkey,
    ) -> Result<()> {
        require!(new_relayer != Pubkey::default(), ErrorCode::ZeroAddress);

        let op_hash = compute_op_hashv(&[
            b"rotateRelayer",
            &old_relayer.to_bytes(),
            &new_relayer.to_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        require!(
            !bs.relayers.contains(&new_relayer),
            ErrorCode::RelayerAlreadyExists
        );
        let idx = bs
            .relayers
            .iter()
            .position(|r| *r == old_relayer)
            .ok_or(error!(ErrorCode::RelayerNotFound))?;
        bs.relayers[idx] = new_relayer;

        emit!(RelayerRemoved {
            relayer: old_relayer
        });
        emit!(RelayerAdded {
            relayer: new_relayer
        });
        Ok(())
    }

    // ─── 管理员：角色管理 ────────────────────────────────────────────────

    /// 提议新管理员（两步转移的第 1 步）。
    pub fn propose_admin(ctx: Context<AdminOp>, new_admin: Pubkey) -> Result<()> {
        require!(new_admin != Pubkey::default(), ErrorCode::ZeroAddress);

        // 提前拒绝与现有角色重叠的提议，避免 timelock 调度被白白消耗，
        // 以及 pending_admin 因 RoleOverlap 永远卡死在 accept_admin 阶段
        {
            let bs_ref = &ctx.accounts.bridge_state;
            require!(
                new_admin != bs_ref.admin
                    && new_admin != bs_ref.guardian
                    && new_admin != bs_ref.operator
                    && new_admin != bs_ref.recovery,
                ErrorCode::RoleOverlap
            );
        }

        let op_hash = compute_op_hashv(&[
            b"proposeAdmin",
            &new_admin.to_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        let current = bs.admin;
        bs.pending_admin = new_admin;

        emit!(AdminTransferProposed {
            current_admin: current,
            pending_admin: new_admin,
        });
        Ok(())
    }

    /// 接受管理员转移（第 2 步）。仅 pending_admin 可调用。
    pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
        let bs = &mut ctx.accounts.bridge_state;
        let new_admin = ctx.accounts.new_admin.key();

        require!(
            new_admin != bs.guardian && new_admin != bs.operator && new_admin != bs.recovery,
            ErrorCode::RoleOverlap
        );

        let old_admin = bs.admin;
        bs.admin = new_admin;
        bs.pending_admin = Pubkey::default();

        emit!(AdminTransferAccepted {
            old_admin,
            new_admin,
        });
        Ok(())
    }

    /// 设置守护者地址。新地址不得与其他角色重叠。
    pub fn set_guardian(ctx: Context<AdminOp>, new_guardian: Pubkey) -> Result<()> {
        require!(new_guardian != Pubkey::default(), ErrorCode::ZeroAddress);

        let op_hash = compute_op_hashv(&[
            b"setGuardian",
            &new_guardian.to_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        // 含 pending_admin：避免新 guardian 与待激活 admin 重叠，
        // 否则会导致 accept_admin 因 RoleOverlap 永远无法完成
        require!(
            new_guardian != bs.admin
                && new_guardian != bs.operator
                && new_guardian != bs.recovery
                && new_guardian != bs.pending_admin,
            ErrorCode::RoleOverlap
        );

        let old = bs.guardian;
        bs.guardian = new_guardian;

        emit!(GuardianUpdated {
            old_guardian: old,
            new_guardian,
        });
        Ok(())
    }

    /// 设置运维者地址。新地址不得与其他角色重叠。
    pub fn set_operator(ctx: Context<AdminOp>, new_operator: Pubkey) -> Result<()> {
        require!(new_operator != Pubkey::default(), ErrorCode::ZeroAddress);

        let op_hash = compute_op_hashv(&[
            b"setOperator",
            &new_operator.to_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        require!(
            new_operator != bs.admin
                && new_operator != bs.guardian
                && new_operator != bs.recovery
                && new_operator != bs.pending_admin,
            ErrorCode::RoleOverlap
        );

        let old = bs.operator;
        bs.operator = new_operator;

        emit!(OperatorUpdated {
            old_operator: old,
            new_operator,
        });
        Ok(())
    }

    /// 设置恢复地址。新地址不得与其他角色重叠。
    pub fn set_recovery(ctx: Context<AdminOp>, new_recovery: Pubkey) -> Result<()> {
        require!(new_recovery != Pubkey::default(), ErrorCode::ZeroAddress);

        let op_hash = compute_op_hashv(&[
            b"setRecovery",
            &new_recovery.to_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        require!(
            new_recovery != bs.admin
                && new_recovery != bs.guardian
                && new_recovery != bs.operator
                && new_recovery != bs.pending_admin,
            ErrorCode::RoleOverlap
        );

        let old = bs.recovery;
        bs.recovery = new_recovery;

        emit!(RecoveryUpdated {
            old_recovery: old,
            new_recovery,
        });
        Ok(())
    }

    // ─── 紧急冻结 / 恢复 ────────────────────────────────────────────────

    /// Guardian 紧急冻结桥，暂停所有 stake 和 unlock 操作。
    pub fn emergency_freeze(ctx: Context<GuardianFreeze>) -> Result<()> {
        ctx.accounts.bridge_state.is_paused = true;
        emit!(EmergencyFreezeActivated {
            triggered_by: ctx.accounts.guardian.key(),
        });
        Ok(())
    }

    /// Recovery 恢复桥：更换 admin、可选替换 guardian、解除冻结。
    pub fn execute_recovery(
        ctx: Context<ExecuteRecovery>,
        new_admin: Pubkey,
        new_guardian: Pubkey,
    ) -> Result<()> {
        require!(new_admin != Pubkey::default(), ErrorCode::ZeroAddress);

        let bs = &mut ctx.accounts.bridge_state;
        let final_guardian = if new_guardian != Pubkey::default() {
            new_guardian
        } else {
            bs.guardian
        };

        require!(
            new_admin != final_guardian
                && new_admin != bs.operator
                && new_admin != bs.recovery,
            ErrorCode::RoleOverlap
        );
        if new_guardian != Pubkey::default() {
            require!(
                new_guardian != bs.operator && new_guardian != bs.recovery,
                ErrorCode::RoleOverlap
            );
        }

        let old_admin = bs.admin;
        bs.admin = new_admin;
        bs.pending_admin = Pubkey::default();

        if new_guardian != Pubkey::default() {
            let old_guardian = bs.guardian;
            bs.guardian = new_guardian;
            emit!(GuardianUpdated {
                old_guardian,
                new_guardian,
            });
        }

        bs.is_paused = false;
        emit!(RecoveryExecuted {
            old_admin,
            new_admin,
        });
        Ok(())
    }

    // ─── 质押 ────────────────────────────────────────────────────────────

    /// 将 USDC 锁入桥金库，发起跨链转移（多 Peer 版本）。
    ///
    /// 流程：
    /// 1. 通过 target_chain_id 查找 PeerConfig PDA 获取目标链配置
    /// 2. CPI 调用 transfer_checked 从用户转入金库
    /// 3. reload 金库余额，用差值计算实际到账金额（兼容 fee-on-transfer 代币）
    /// 4. 扣除 peer_config.bridge_fee 得到事件净额（留在金库作为协议收入）
    /// 5. 创建 StakeRecord PDA 记录 owner、amount 和 target_chain_id（用于退款）
    /// 6. emit Staked 供中继器监听
    pub fn stake(
        ctx: Context<StakeAccounts>,
        nonce: u64,
        amount: u64,
        receiver: [u8; 32],
        target_chain_id: u64,
    ) -> Result<u64> {
        require!(receiver != [0u8; 32], ErrorCode::ZeroAddress);

        let bs = &ctx.accounts.bridge_state;
        let pc = &ctx.accounts.peer_config;
        // PDA seeds 已经隐式约束 target_chain_id == pc.chain_id，
        // 这里再显式断言一次以在 IDL 中保留该参数，并提供比 ConstraintSeeds 更明确的错误码
        require!(target_chain_id == pc.chain_id, ErrorCode::InvalidChainId);
        require!(amount > 0, ErrorCode::ZeroAmount);

        let vault_balance_before = ctx.accounts.vault_token_account.amount;

        let cpi_accounts = TransferChecked {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
            mint: ctx.accounts.usdc_mint.to_account_info(),
        };
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
            ),
            amount,
            ctx.accounts.usdc_mint.decimals,
        )?;

        ctx.accounts.vault_token_account.reload()?;
        let actual_amount = ctx
            .accounts
            .vault_token_account
            .amount
            .checked_sub(vault_balance_before)
            .ok_or(error!(ErrorCode::InsufficientBalance))?;
        require!(actual_amount > 0, ErrorCode::ZeroAmount);
        if pc.max_stake_amount != 0 {
            require!(
                actual_amount <= pc.max_stake_amount,
                ErrorCode::StakeAmountExceeded
            );
        }

        let event_amount = actual_amount
            .checked_sub(pc.bridge_fee)
            .ok_or_else(|| error!(ErrorCode::FeeExceedsAmount))?;
        require!(event_amount > 0, ErrorCode::FeeExceedsAmount);

        let stake_record = &mut ctx.accounts.stake_record;
        stake_record.owner = ctx.accounts.user.key();
        stake_record.amount = actual_amount;
        stake_record.target_chain_id = pc.chain_id;

        let clock = Clock::get()?;
        emit!(Staked {
            source_contract: crate::ID.to_bytes(),
            target_contract: pc.peer_contract,
            source_chain_id: bs.local_chain_id,
            target_chain_id: pc.chain_id,
            block_height: clock.slot,
            raw_amount: actual_amount,
            amount: event_amount,
            sender: ctx.accounts.user.key().to_bytes(),
            receiver,
            nonce,
        });

        Ok(nonce)
    }

    // ─── 确认事件（哈希投票） ────────────────────────────────────────────

    /// 中继器确认跨链事件（多 Peer 版本，投票机制）。
    ///
    /// 每个中继器提交完整的 event_data，合约对数据取 SHA-256 哈希后投票计数。
    /// 当同一哈希的投票数达到 frozen_threshold 时自动触发 USDC 解锁转账。
    /// 源链 stake 时已扣除 bridge_fee，此处全额转 event_data.amount 给用户。
    ///
    /// 多 Peer 变更：
    /// - 通过 source_chain_id 派生 PeerConfig PDA，校验来源链路
    /// - CrossChainRequest PDA seeds 加入 source_chain_id 隔离 nonce 空间
    /// - unlock 时执行双层速率检查（per-chain + 全局）
    pub fn confirm_event(
        ctx: Context<ConfirmEvent>,
        nonce: u64,
        source_chain_id: u64,
        event_data: BridgeEventData,
    ) -> Result<()> {
        let bs = &mut ctx.accounts.bridge_state;
        let pc = &mut ctx.accounts.peer_config;
        let req = &mut ctx.accounts.cross_chain_request;

        // ── 幂等性 + 参数一致性检查（最廉价，优先前置） ──
        require!(!req.is_processed, ErrorCode::AlreadyProcessed);
        require!(nonce == event_data.nonce, ErrorCode::NonceMismatch);
        require!(
            source_chain_id == event_data.source_chain_id,
            ErrorCode::SourceChainIdMismatch
        );
        require!(event_data.amount > 0, ErrorCode::ZeroAmount);

        // 提前拒绝超出 per-chain / 全局单笔限额的事件，避免 relayer 浪费 CU
        if pc.max_single_unlock != 0 && event_data.amount > pc.max_single_unlock {
            return err!(ErrorCode::SingleTransferExceeded);
        }
        if bs.max_single_unlock != 0 && event_data.amount > bs.max_single_unlock {
            return err!(ErrorCode::SingleTransferExceeded);
        }

        // ── 跨链地址/链 ID 校验（使用 PeerConfig） ──
        require!(
            event_data.target_contract == crate::ID.to_bytes(),
            ErrorCode::InvalidTargetContract
        );
        require!(
            event_data.source_contract == pc.peer_contract,
            ErrorCode::InvalidSourceContract
        );
        require!(
            event_data.source_chain_id == pc.chain_id,
            ErrorCode::InvalidSourceChainId
        );
        require!(
            event_data.target_chain_id == bs.local_chain_id,
            ErrorCode::InvalidTargetChainId
        );

        // ── receiver 校验 ──
        let receiver_key = Pubkey::new_from_array(event_data.receiver);
        require!(receiver_key != Pubkey::default(), ErrorCode::ZeroAddress);
        require!(
            ctx.accounts.receiver_token_account.owner == receiver_key,
            ErrorCode::InvalidReceiver
        );

        // ── 中继器身份与去重检查 ──
        let relayer_key = ctx.accounts.relayer.key();
        require!(bs.is_relayer(&relayer_key), ErrorCode::RelayerNotFound);
        require!(
            !req.confirmed_relayers.contains(&relayer_key),
            ErrorCode::RelayerAlreadyConfirmed
        );

        // ── 首个中继器初始化请求状态 ──
        if req.confirmed_relayers.is_empty() {
            req.nonce = event_data.nonce;
            req.frozen_threshold = ((bs.relayers.len() as u16 * 2 + 2) / 3) as u8;
        }

        req.confirmed_relayers.push(relayer_key);

        // ── 计算事件数据哈希（零堆分配） ──
        let src_chain = event_data.source_chain_id.to_le_bytes();
        let tgt_chain = event_data.target_chain_id.to_le_bytes();
        let height = event_data.block_height.to_le_bytes();
        let raw_amt = event_data.raw_amount.to_le_bytes();
        let amt = event_data.amount.to_le_bytes();
        let nonce_bytes = event_data.nonce.to_le_bytes();
        let data_hash: [u8; 32] = solana_sha256_hasher::hashv(&[
            &event_data.source_contract,
            &event_data.target_contract,
            &src_chain,
            &tgt_chain,
            &height,
            &raw_amt,
            &amt,
            &event_data.sender,
            &event_data.receiver,
            &nonce_bytes,
        ]).to_bytes();

        // ── 哈希投票 ──
        let mut winning_count: u8 = 0;
        let mut vote_found = false;
        for vote in req.hash_votes.iter_mut() {
            if vote.data_hash == data_hash {
                vote.count = vote.count
                    .checked_add(1)
                    .ok_or_else(|| error!(ErrorCode::NonceOverflow))?;
                winning_count = vote.count;
                vote_found = true;
                break;
            }
        }
        if !vote_found {
            req.hash_votes.push(HashVote {
                data_hash,
                count: 1,
            });
            winning_count = 1;
        }

        emit!(EventConfirmed {
            relayer: relayer_key,
            nonce: event_data.nonce,
            data_hash,
        });

        // ── 达到阈值：触发解锁（源链 stake 时已扣 bridge_fee，此处全额转给用户） ──
        if winning_count >= req.frozen_threshold && !req.is_unlocked {
            let unlock_amount = event_data.amount;

            check_dual_transfer_limits(bs, pc, unlock_amount)?;

            check_vault_invariant(
                ctx.accounts.vault_token_account.amount,
                unlock_amount,
                bs.minimum_reserve,
            )?;

            req.is_unlocked = true;
            req.is_processed = true;
            req.confirmed_relayers.clear();
            req.hash_votes.clear();

            let vault_bump = bs.vault_bump;
            let signer_seeds: &[&[&[u8]]] = &[&[b"vault", &[vault_bump]]];

            let cpi_accounts = TransferChecked {
                from: ctx.accounts.vault_token_account.to_account_info(),
                to: ctx.accounts.receiver_token_account.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
            };
            token_interface::transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    cpi_accounts,
                    signer_seeds,
                ),
                unlock_amount,
                ctx.accounts.usdc_mint.decimals,
            )?;

            compact_request_pda(
                &req.to_account_info(),
                &ctx.accounts.relayer.to_account_info(),
            )?;

            emit!(Unlocked {
                source_contract: event_data.source_contract,
                target_contract: event_data.target_contract,
                source_chain_id: event_data.source_chain_id,
                target_chain_id: event_data.target_chain_id,
                block_height: event_data.block_height,
                raw_amount: event_data.raw_amount,
                amount: unlock_amount,
                sender: event_data.sender,
                receiver: event_data.receiver,
                nonce: event_data.nonce,
            });
        }

        Ok(())
    }

    // ─── 操作员：跳过 / 退款 ────────────────────────────────────────────

    /// 操作员将某个 nonce 标记为"跳过"（接收端使用，多 Peer 版本）。
    ///
    /// 需要指定 source_chain_id 以匹配 CrossChainRequest PDA 的 seeds。
    pub fn skip_nonce(ctx: Context<SkipNonce>, nonce: u64, source_chain_id: u64) -> Result<()> {
        // source_chain_id 的一致性由 PDA seeds 在 Anchor 派生阶段强制，
        // 不再做冗余的 require! 断言；参数本身随 NonceSkipped 事件输出，
        // 便于链下索引器区分不同对端链的 nonce 空间
        {
            let req = &mut ctx.accounts.cross_chain_request;
            require!(!req.is_processed, ErrorCode::AlreadyProcessed);
            req.is_processed = true;
            req.nonce = nonce;
            req.confirmed_relayers.clear();
            req.hash_votes.clear();
        }

        compact_request_pda(
            &ctx.accounts.cross_chain_request.to_account_info(),
            &ctx.accounts.operator.to_account_info(),
        )?;

        emit!(NonceSkipped {
            nonce,
            source_chain_id,
        });
        Ok(())
    }

    /// 发起退款（两步退款的第 1 步，仅 operator 可调用）。
    pub fn initiate_refund(ctx: Context<InitiateRefund>, nonce: u64) -> Result<()> {
        let stake_record = &mut ctx.accounts.stake_record;
        let clock = Clock::get()?;
        stake_record.refund_initiated_at = clock.unix_timestamp as u64;

        emit!(RefundInitiated {
            nonce,
            to: stake_record.owner,
            amount: stake_record.amount,
        });
        Ok(())
    }

    /// 执行退款（两步退款的第 2 步，operator 或原始 staker 均可调用）。
    ///
    /// 退款只走全局速率限制（不涉及 peer 链路出金），受金库最低储备约束。
    pub fn execute_refund(ctx: Context<ExecuteRefund>, nonce: u64) -> Result<()> {
        let bs = &mut ctx.accounts.bridge_state;
        let stake_record = &mut ctx.accounts.stake_record;

        let clock = Clock::get()?;
        let now = clock.unix_timestamp as u64;
        require!(
            now >= stake_record.refund_initiated_at
                .checked_add(REFUND_DELAY)
                .ok_or_else(|| error!(ErrorCode::RefundNotReady))?,
            ErrorCode::RefundNotReady
        );

        let amount = stake_record.amount;

        check_global_transfer_limits(bs, amount)?;
        check_vault_invariant(
            ctx.accounts.vault_token_account.amount,
            amount,
            bs.minimum_reserve,
        )?;

        stake_record.refunded = true;
        stake_record.refund_initiated_at = 0;

        let vault_bump = bs.vault_bump;
        let signer_seeds: &[&[&[u8]]] = &[&[b"vault", &[vault_bump]]];

        let cpi_accounts = TransferChecked {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.owner_token_account.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
            mint: ctx.accounts.usdc_mint.to_account_info(),
        };
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
                signer_seeds,
            ),
            amount,
            ctx.accounts.usdc_mint.decimals,
        )?;

        emit!(Refunded {
            nonce,
            to: stake_record.owner,
            amount,
        });
        Ok(())
    }

    /// 取消已发起的退款（仅 admin 可调用，暂停时也可调用）。
    pub fn cancel_refund(ctx: Context<CancelRefund>, nonce: u64) -> Result<()> {
        let stake_record = &mut ctx.accounts.stake_record;
        stake_record.refund_initiated_at = 0;

        emit!(RefundCancelled { nonce });
        Ok(())
    }

    // ─── 管理员：提取 ────────────────────────────────────────────────────

    /// 管理员从金库提取代币（受时间锁保护）。
    pub fn withdraw_token(ctx: Context<WithdrawToken>, amount: u64, to: Pubkey) -> Result<()> {
        require!(amount > 0, ErrorCode::ZeroAmount);

        let op_hash = compute_op_hashv(&[
            b"withdrawToken",
            &ctx.accounts.token_mint.key().to_bytes(),
            &amount.to_le_bytes(),
            &to.to_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let vault_bump = ctx.accounts.bridge_state.vault_bump;
        let signer_seeds: &[&[&[u8]]] = &[&[b"vault", &[vault_bump]]];

        let cpi_accounts = TransferChecked {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.to_token_account.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
            mint: ctx.accounts.token_mint.to_account_info(),
        };
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
                signer_seeds,
            ),
            amount,
            ctx.accounts.token_mint.decimals,
        )?;

        emit!(TokenWithdrawn {
            mint: ctx.accounts.token_mint.key(),
            to,
            amount,
        });
        Ok(())
    }

}
