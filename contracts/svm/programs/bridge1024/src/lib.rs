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

declare_id!("59j1516cVrj3dpfVe7zQWrTwufajBNdJ5rYvJH7N2hq1");

/// 硬编码的初始管理员地址（2XVdXwC235qFXSm5egXpWyNY9xaiShFD5HKGrEhQNEFY）。
/// 部署前必须设置为实际部署者的公钥。
/// 防止 initialize 被抢先调用（front-running），比 verify_upgrade_authority 更可靠，
/// 无 Solana 版本兼容性问题。
pub const INITIAL_ADMIN: Pubkey = Pubkey::new_from_array([
    22, 171, 123, 173, 77, 255, 198, 3, 77, 94, 188, 132, 148, 188, 245, 57,
    58, 135, 108, 181, 100, 2, 76, 171, 21, 38, 157, 187, 65, 193, 151, 151,
]);

/// Bridge1024 SVM 跨链桥程序。
///
/// 本程序是 Bridge1024 跨链桥的 Solana 端实现，支持 stake（锁定）和 unlock（解锁）两种核心操作。
/// 用户在源链 stake USDC 后，中继器（relayer）在目标链提交确认，达到 2/3 投票阈值后自动触发 unlock。
///
/// 核心流程：
/// - 出金：用户 stake → 中继器监听 StakeEvent → 在对端链 confirm_event → 达到阈值自动 unlock
/// - 异常：operator skip_nonce（接收端）→ operator initiate_refund → execute_refund（发送端）
///
/// 安全机制：
/// - 四角色分离（admin / guardian / operator / recovery）
/// - 时间锁（24h 延迟 + 48h 执行窗口）
/// - 滑动窗口速率限制
/// - 金库最低储备金
/// - 白名单中继器哈希投票（2/3 阈值）
/// - 紧急冻结与恢复
///
/// SVM 特有功能：
/// - 双向手续费（stake 和 unlock 时都扣除 bridge_fee）
/// - vault_bump 缓存（避免重复 find_program_address）
/// - Token-2022（token_interface）兼容
#[program]
pub mod bridge1024 {
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
        bs.vault = ctx.accounts.vault.key();
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

    // ─── 管理员：配置 ────────────────────────────────────────────────────

    /// 设置 USDC 铸币地址、对端合约地址和链 ID。
    ///
    /// 这些参数在部署后通常只设置一次，合并为单个函数以减少交易次数并保证原子性。
    /// ⚠️ 修改 peer_contract 或链 ID 会导致所有进行中的 CrossChainRequest 因校验不匹配而永久卡住，
    /// 受影响的 nonce 需通过 skip_nonce + initiate_refund/execute_refund 流程处理退款。
    pub fn configure(
        ctx: Context<AdminOp>,
        usdc_mint: Pubkey,
        peer_contract: [u8; 32],
        local_chain_id: u64,
        peer_chain_id: u64,
    ) -> Result<()> {
        require!(usdc_mint != Pubkey::default(), ErrorCode::ZeroAddress);
        require!(peer_contract != [0u8; 32], ErrorCode::ZeroAddress);
        require!(
            local_chain_id != 0 && peer_chain_id != 0,
            ErrorCode::InvalidChainId
        );
        require!(local_chain_id != peer_chain_id, ErrorCode::InvalidChainId);

        let op_hash = compute_op_hashv(&[
            b"configure",
            &usdc_mint.to_bytes(),
            &peer_contract,
            &local_chain_id.to_le_bytes(),
            &peer_chain_id.to_le_bytes(),
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
        bs.peer_contract = peer_contract;
        bs.local_chain_id = local_chain_id;
        bs.peer_chain_id = peer_chain_id;

        emit!(BridgeConfigured {
            usdc_mint,
            peer_contract,
            local_chain_id,
            peer_chain_id,
        });
        Ok(())
    }

    /// 原子性设置所有速率限制参数，同时重置滑动窗口。
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
        max_stake: u64,
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
            &max_stake.to_le_bytes(),
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
        bs.max_stake_amount = max_stake;
        bs.minimum_reserve = min_reserve;
        let clock = Clock::get()?;
        bs.current_window_start = clock.unix_timestamp as u64;
        bs.current_window_usage = 0;
        bs.previous_window_usage = 0;

        emit!(RateLimitsConfigured {
            max_unlock_per_window: max_per_window,
            window_duration,
            max_single_unlock: max_single,
            max_stake_amount: max_stake,
            minimum_reserve: min_reserve,
        });
        Ok(())
    }

    /// 设置桥手续费（SVM 特有）。
    ///
    /// 在 stake（发送端）和 unlock（接收端）时都会扣除，扣除的手续费留在金库作为协议收入。
    /// fee 不得超过 MAX_FEE（1000 USDC），防止管理员误操作。
    pub fn configure_fee(ctx: Context<AdminOp>, fee: u64) -> Result<()> {
        require!(fee <= MAX_FEE, ErrorCode::FeeTooHigh);

        let op_hash = compute_op_hashv(&[
            b"configureFee",
            &fee.to_le_bytes(),
        ]);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        ctx.accounts.bridge_state.bridge_fee = fee;
        emit!(FeeConfigured { fee });
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
    ///
    /// 新管理员必须主动调用 accept_admin 接受，确保新管理员确实控制该地址。
    /// 如需取消提议，使用 cancel_operation 取消对应的 timelock 调度。
    pub fn propose_admin(ctx: Context<AdminOp>, new_admin: Pubkey) -> Result<()> {
        require!(new_admin != Pubkey::default(), ErrorCode::ZeroAddress);

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
    ///
    /// 新管理员不得与其他角色重叠，完成后清空 pending_admin。
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
        require!(
            new_guardian != bs.admin
                && new_guardian != bs.operator
                && new_guardian != bs.recovery,
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
                && new_operator != bs.recovery,
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
                && new_recovery != bs.operator,
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
    ///
    /// 冻结后 admin 无法解除，只有 recovery 地址可通过 execute_recovery 恢复。
    /// Guardian 泄露的最坏情况是 DoS（需 recovery 解冻），不会丢失资金。
    pub fn emergency_freeze(ctx: Context<GuardianFreeze>) -> Result<()> {
        ctx.accounts.bridge_state.is_paused = true;
        emit!(EmergencyFreezeActivated {
            triggered_by: ctx.accounts.guardian.key(),
        });
        Ok(())
    }

    /// Recovery 恢复桥：更换 admin、可选替换 guardian、解除冻结。
    ///
    /// 仅在紧急冻结状态下可调用，确保 recovery 地址不能在正常状态下越权。
    /// 允许同时替换 guardian 以打破恶意 guardian 反复冻结的 DoS 循环：
    /// 若仅替换 admin，新 admin 需通过 set_guardian（24h timelock）才能换掉恶意 guardian，
    /// 在此期间恶意 guardian 可不断 freeze→recovery→freeze，造成持续服务中断。
    ///
    /// new_guardian 传 Pubkey::default() 表示保留当前 guardian。
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

    /// 将 USDC 锁入桥金库，发起跨链转移。
    ///
    /// 流程：
    /// 1. CPI 调用 transfer_checked 从用户转入金库
    /// 2. reload 金库余额，用差值计算实际到账金额（兼容 fee-on-transfer 代币）
    /// 3. 扣除 bridge_fee 得到事件净额（留在金库作为协议收入）
    /// 4. 创建 StakeRecord PDA 记录 owner 和 amount（用于退款）
    /// 5. emit StakeEvent 供中继器监听
    ///
    /// nonce 由客户端生成随机值传入，PDA 的 init 约束天然防止碰撞。
    /// 随机 nonce 消除全局串行瓶颈，并防止 operator 泄露后预测性 skip_nonce DoS。
    pub fn stake(
        ctx: Context<StakeAccounts>,
        nonce: u64,
        amount: u64,
        receiver: [u8; 32],
    ) -> Result<u64> {
        require!(receiver != [0u8; 32], ErrorCode::ZeroAddress);

        let bs = &mut ctx.accounts.bridge_state;
        require!(amount > 0, ErrorCode::ZeroAmount);

        // 记录转账前金库余额，转账后用差值计算实际到账金额
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

        // reload 获取最新余额，差值即为实际到账金额（兼容 fee-on-transfer）
        ctx.accounts.vault_token_account.reload()?;
        let actual_amount = ctx
            .accounts
            .vault_token_account
            .amount
            .checked_sub(vault_balance_before)
            .ok_or(error!(ErrorCode::InsufficientBalance))?;
        require!(actual_amount > 0, ErrorCode::ZeroAmount);
        if bs.max_stake_amount != 0 {
            require!(
                actual_amount <= bs.max_stake_amount,
                ErrorCode::StakeAmountExceeded
            );
        }

        // 扣除手续费得到事件净额，手续费留在金库作为协议收入
        let event_amount = actual_amount
            .checked_sub(bs.bridge_fee)
            .ok_or_else(|| error!(ErrorCode::FeeExceedsAmount))?;
        require!(event_amount > 0, ErrorCode::FeeExceedsAmount);

        // StakeRecord 记录用户实付全额（含手续费），退款时退还全额
        // StakeEvent.amount 使用扣费后净额，用于对端链 unlock
        let stake_record = &mut ctx.accounts.stake_record;
        stake_record.owner = ctx.accounts.user.key();
        stake_record.amount = actual_amount;

        let clock = Clock::get()?;
        emit!(StakeEvent {
            source_contract: crate::ID.to_bytes(),
            target_contract: bs.peer_contract,
            source_chain_id: bs.local_chain_id,
            target_chain_id: bs.peer_chain_id,
            block_height: clock.slot,
            amount: event_amount,
            sender: ctx.accounts.user.key().to_bytes(),
            receiver,
            nonce,
        });

        Ok(nonce)
    }

    // ─── 确认事件（哈希投票） ────────────────────────────────────────────

    /// 中继器确认跨链事件（投票机制）。
    ///
    /// 每个中继器提交完整的 event_data，合约对数据取 SHA-256 哈希后投票计数。
    /// 当同一哈希的投票数达到 frozen_threshold（首次确认时冻结的 2/3 阈值）时，
    /// 自动触发 USDC 解锁转账。
    ///
    /// 哈希投票的优势：少数中继器提交错误数据不影响正常流程，多数正确即可通过。
    ///
    /// 哈希计算使用 hashv 逐字段散列，与 Borsh try_to_vec() 产生相同的字节序列，
    /// 但避免在 BPF 堆上分配 Vec，节省 CU。
    ///
    /// 中继器身份由 Solana 原生交易签名者校验（等价于 EVM 的 msg.sender），
    /// 白名单检查通过 bridge_state.is_relayer() 完成。
    pub fn confirm_event(
        ctx: Context<ConfirmEvent>,
        _nonce: u64,
        event_data: StakeEventData,
    ) -> Result<()> {
        let bs = &mut ctx.accounts.bridge_state;

        // ── 基础校验 ──
        require!(event_data.amount > bs.bridge_fee, ErrorCode::FeeExceedsAmount);
        require!(_nonce == event_data.nonce, ErrorCode::NonceMismatch);

        // ── 跨链地址/链 ID 校验 ──
        require!(
            event_data.target_contract == crate::ID.to_bytes(),
            ErrorCode::InvalidTargetContract
        );
        require!(
            event_data.source_contract == bs.peer_contract,
            ErrorCode::InvalidSourceContract
        );
        require!(
            event_data.source_chain_id == bs.peer_chain_id,
            ErrorCode::InvalidSourceChainId
        );
        require!(
            event_data.target_chain_id == bs.local_chain_id,
            ErrorCode::InvalidTargetChainId
        );

        // ── receiver 校验（所有投票统一验证，而非仅在解锁时） ──
        let receiver_key = Pubkey::new_from_array(event_data.receiver);
        require!(receiver_key != Pubkey::default(), ErrorCode::ZeroAddress);
        require!(
            ctx.accounts.receiver_token_account.owner == receiver_key,
            ErrorCode::InvalidReceiver
        );

        let req = &mut ctx.accounts.cross_chain_request;
        require!(!req.is_processed, ErrorCode::AlreadyProcessed);

        // ── 中继器身份与去重检查 ──
        let relayer_key = ctx.accounts.relayer.key();
        require!(bs.is_relayer(&relayer_key), ErrorCode::RelayerNotFound);
        require!(
            !req.confirmed_relayers.contains(&relayer_key),
            ErrorCode::RelayerAlreadyConfirmed
        );

        // ── 首个中继器初始化请求状态 ──
        // frozen_threshold 在此时冻结：即使后续 relayer 数量变化，
        // 进行中的投票阈值不受影响，防止管理员通过增减 relayer 操纵投票结果
        if req.confirmed_relayers.is_empty() {
            req.nonce = event_data.nonce;
            req.frozen_threshold = ((bs.relayers.len() as u16 * 2 + 2) / 3) as u8;
            req.hash_votes = Vec::new();
        }

        req.confirmed_relayers.push(relayer_key);

        // ── 计算事件数据哈希（零堆分配） ──
        let src_chain = event_data.source_chain_id.to_le_bytes();
        let tgt_chain = event_data.target_chain_id.to_le_bytes();
        let height = event_data.block_height.to_le_bytes();
        let amt = event_data.amount.to_le_bytes();
        let nonce_bytes = event_data.nonce.to_le_bytes();
        let data_hash: [u8; 32] = solana_sha256_hasher::hashv(&[
            &event_data.source_contract,
            &event_data.target_contract,
            &src_chain,
            &tgt_chain,
            &height,
            &amt,
            &event_data.sender,
            &event_data.receiver,
            &nonce_bytes,
        ]).to_bytes();

        // ── 哈希投票：查找已有桶或创建新桶 ──
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
        });

        // ── 达到阈值：触发解锁 ──
        if winning_count >= req.frozen_threshold && !req.is_unlocked {
            let net_amount = event_data.amount
                .checked_sub(bs.bridge_fee)
                .ok_or_else(|| error!(ErrorCode::FeeExceedsAmount))?;
            require!(net_amount > 0, ErrorCode::FeeExceedsAmount);

            check_transfer_limits(bs, net_amount)?;

            check_vault_invariant(
                ctx.accounts.vault_token_account.amount,
                net_amount,
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
                net_amount,
                ctx.accounts.usdc_mint.decimals,
            )?;

            // 自动压缩：缩小 PDA、退还多余租金给触发 unlock 的 relayer
            compact_request_pda(
                &req.to_account_info(),
                &ctx.accounts.relayer.to_account_info(),
            )?;

            emit!(TokensUnlocked {
                nonce: event_data.nonce,
                receiver: receiver_key,
                amount: net_amount,
                sender: event_data.sender,
            });
        }

        Ok(())
    }

    // ─── 操作员：跳过 / 退款 ────────────────────────────────────────────

    /// 操作员将某个 nonce 标记为"跳过"（接收端使用）。
    ///
    /// 封死该 nonce 的解锁可能，配合发送端退款流程退还用户资金。
    /// 状态设为 Skipped（非 Unlocked），链下系统可据此区分解锁与退款。
    /// ⚠️ 必须在对端链 initiate_refund 之前调用，否则存在双花风险。
    pub fn skip_nonce(ctx: Context<SkipNonce>, nonce: u64) -> Result<()> {
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

        emit!(NonceSkipped { nonce });
        Ok(())
    }

    /// 发起退款（两步退款的第 1 步，仅 operator 可调用）。
    ///
    /// 记录发起时间戳，需等待 REFUND_DELAY 后才能执行第 2 步。
    /// 延迟期间 admin 可通过 cancel_refund 取消。
    /// 防止 operator 密钥泄露后立即退款造成双花。
    pub fn initiate_refund(ctx: Context<InitiateRefund>, _nonce: u64) -> Result<()> {
        let stake_record = &mut ctx.accounts.stake_record;
        let clock = Clock::get()?;
        stake_record.refund_initiated_at = clock.unix_timestamp as u64;

        emit!(RefundInitiated {
            nonce: _nonce,
            to: stake_record.owner,
            amount: stake_record.amount,
        });
        Ok(())
    }

    /// 执行退款（两步退款的第 2 步，operator 或原始 staker 均可调用）。
    ///
    /// 需等待 REFUND_DELAY 后方可执行，受速率限制和金库最低储备约束。
    /// ⚠️ 必须先在对端链 skip_nonce 封死 unlock，再发起退款，否则存在双花风险。
    pub fn execute_refund(ctx: Context<ExecuteRefund>, _nonce: u64) -> Result<()> {
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

        check_transfer_limits(bs, amount)?;
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
            nonce: _nonce,
            to: stake_record.owner,
            amount,
        });
        Ok(())
    }

    /// 取消已发起的退款（仅 admin 可调用，暂停时也可调用）。
    ///
    /// 用于 operator 密钥泄露后阻止恶意退款执行。
    pub fn cancel_refund(ctx: Context<CancelRefund>, _nonce: u64) -> Result<()> {
        let stake_record = &mut ctx.accounts.stake_record;
        stake_record.refund_initiated_at = 0;

        emit!(RefundCancelled { nonce: _nonce });
        Ok(())
    }

    // ─── 管理员：提取 ────────────────────────────────────────────────────

    /// 管理员从金库提取代币（受时间锁保护）。
    /// 用于处理误转入的代币或按需转移资金。
    pub fn withdraw_token(ctx: Context<WithdrawToken>, amount: u64, to: Pubkey) -> Result<()> {
        require!(amount > 0, ErrorCode::ZeroAmount);

        let op_hash = compute_op_hashv(&[
            b"withdrawToken",
            &ctx.accounts.usdc_mint.key().to_bytes(),
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

        emit!(TokenWithdrawn {
            mint: ctx.accounts.usdc_mint.key(),
            to,
            amount,
        });
        Ok(())
    }

}
