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

#[program]
pub mod bridge1024 {
    use super::*;

    // ─── 初始化 ──────────────────────────────────────────────────────────

    /// 创建 BridgeState PDA，设置四角色分离。
    /// 所有角色地址必须非零且互不相同。
    pub fn initialize(
        ctx: Context<Initialize>,
        guardian: Pubkey,
        operator: Pubkey,
        recovery: Pubkey,
    ) -> Result<()> {
        let admin = ctx.accounts.admin.key();
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
        bs.pending_admin = Pubkey::default();
        bs.vault = ctx.accounts.vault.key();
        bs.usdc_mint = Pubkey::default();
        bs.peer_contract = [0u8; 32];
        bs.local_chain_id = 0;
        bs.peer_chain_id = 0;
        bs.sender_nonce = 0;
        bs.max_unlock_per_window = 0;
        bs.window_duration = 0;
        bs.current_window_start = 0;
        bs.current_window_usage = 0;
        bs.previous_window_usage = 0;
        bs.max_single_unlock = 0;
        bs.max_stake_amount = 0;
        bs.minimum_reserve = 0;
        bs.bridge_fee = 0;
        bs.is_paused = false;
        bs.timelock_active = false;
        bs.relayer_count = 0;
        bs.relayers = Vec::new();

        Ok(())
    }

    // ─── 时间锁 ──────────────────────────────────────────────────────────

    /// 不可逆地激活时间锁。激活后所有关键管理操作需要：
    /// 调度 → 等待 24 小时 → 在 48 小时窗口内执行。
    pub fn activate_timelock(ctx: Context<ActivateTimelock>) -> Result<()> {
        let bs = &mut ctx.accounts.bridge_state;
        require!(!bs.timelock_active, ErrorCode::TimelockAlreadyActive);
        bs.timelock_active = true;
        emit!(TimelockActivated {});
        Ok(())
    }

    /// 调度一个时间锁操作。PDA 由 `op_hash` 派生。
    /// `data` 为原始操作负载；`op_hash` 必须等于 SHA-256(data)。
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

    /// 取消已调度的操作。桥暂停时也可调用。
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
    /// 时间锁激活后受保护。
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

        let mut data = Vec::new();
        data.extend_from_slice(b"configure");
        data.extend_from_slice(&usdc_mint.to_bytes());
        data.extend_from_slice(&peer_contract);
        data.extend_from_slice(&local_chain_id.to_le_bytes());
        data.extend_from_slice(&peer_chain_id.to_le_bytes());
        let op_hash = compute_op_hash(&data);

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

        let mut data = Vec::new();
        data.extend_from_slice(b"configureRateLimits");
        data.extend_from_slice(&max_per_window.to_le_bytes());
        data.extend_from_slice(&window_duration.to_le_bytes());
        data.extend_from_slice(&max_single.to_le_bytes());
        data.extend_from_slice(&max_stake.to_le_bytes());
        data.extend_from_slice(&min_reserve.to_le_bytes());
        let op_hash = compute_op_hash(&data);

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
    /// 在 stake（发送端）和 unlock（接收端）时都会扣除，SVM 双向收费。
    pub fn configure_fee(ctx: Context<AdminOp>, fee: u64) -> Result<()> {
        require!(fee <= MAX_FEE, ErrorCode::FeeTooHigh);

        let mut data = Vec::new();
        data.extend_from_slice(b"configureFee");
        data.extend_from_slice(&fee.to_le_bytes());
        let op_hash = compute_op_hash(&data);

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

    pub fn add_relayer(ctx: Context<AdminOp>, relayer: Pubkey) -> Result<()> {
        require!(relayer != Pubkey::default(), ErrorCode::ZeroAddress);

        let mut data = Vec::new();
        data.extend_from_slice(b"addRelayer");
        data.extend_from_slice(&relayer.to_bytes());
        let op_hash = compute_op_hash(&data);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let bs = &mut ctx.accounts.bridge_state;
        require!(
            (bs.relayer_count as usize) < MAX_RELAYERS,
            ErrorCode::TooManyRelayers
        );
        require!(
            !bs.relayers.contains(&relayer),
            ErrorCode::RelayerAlreadyExists
        );

        bs.relayers.push(relayer);
        bs.relayer_count += 1;
        emit!(RelayerAdded { relayer });
        Ok(())
    }

    pub fn remove_relayer(ctx: Context<AdminOp>, relayer: Pubkey) -> Result<()> {
        let mut data = Vec::new();
        data.extend_from_slice(b"removeRelayer");
        data.extend_from_slice(&relayer.to_bytes());
        let op_hash = compute_op_hash(&data);

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
        bs.relayer_count -= 1;

        emit!(RelayerRemoved { relayer });
        Ok(())
    }

    pub fn rotate_relayer(
        ctx: Context<AdminOp>,
        old_relayer: Pubkey,
        new_relayer: Pubkey,
    ) -> Result<()> {
        require!(new_relayer != Pubkey::default(), ErrorCode::ZeroAddress);

        let mut data = Vec::new();
        data.extend_from_slice(b"rotateRelayer");
        data.extend_from_slice(&old_relayer.to_bytes());
        data.extend_from_slice(&new_relayer.to_bytes());
        let op_hash = compute_op_hash(&data);

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

        let mut data = Vec::new();
        data.extend_from_slice(b"proposeAdmin");
        data.extend_from_slice(&new_admin.to_bytes());
        let op_hash = compute_op_hash(&data);

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

    pub fn set_guardian(ctx: Context<AdminOp>, new_guardian: Pubkey) -> Result<()> {
        require!(new_guardian != Pubkey::default(), ErrorCode::ZeroAddress);

        let mut data = Vec::new();
        data.extend_from_slice(b"setGuardian");
        data.extend_from_slice(&new_guardian.to_bytes());
        let op_hash = compute_op_hash(&data);

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

    pub fn set_operator(ctx: Context<AdminOp>, new_operator: Pubkey) -> Result<()> {
        require!(new_operator != Pubkey::default(), ErrorCode::ZeroAddress);

        let mut data = Vec::new();
        data.extend_from_slice(b"setOperator");
        data.extend_from_slice(&new_operator.to_bytes());
        let op_hash = compute_op_hash(&data);

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

    pub fn set_recovery(ctx: Context<AdminOp>, new_recovery: Pubkey) -> Result<()> {
        require!(new_recovery != Pubkey::default(), ErrorCode::ZeroAddress);

        let mut data = Vec::new();
        data.extend_from_slice(b"setRecovery");
        data.extend_from_slice(&new_recovery.to_bytes());
        let op_hash = compute_op_hash(&data);

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

    /// 仅监护人可调用：立即暂停桥。只有恢复者可以解除暂停。
    pub fn emergency_freeze(ctx: Context<GuardianFreeze>) -> Result<()> {
        ctx.accounts.bridge_state.is_paused = true;
        emit!(EmergencyFreezeActivated {
            triggered_by: ctx.accounts.guardian.key(),
        });
        Ok(())
    }

    /// 仅恢复者可调用（桥暂停时）：替换管理员，可选替换监护人，并解除暂停。
    /// new_guardian 传 Pubkey::default() 表示保留当前监护人。
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

    /// 将 USDC 锁入桥金库。创建 StakeRecord PDA 以支持退款。
    /// 发送端扣除手续费（留在金库作为协议收入）。
    pub fn stake(
        ctx: Context<StakeAccounts>,
        nonce: u64,
        amount: u64,
        receiver: [u8; 32],
    ) -> Result<u64> {
        require!(amount > 0, ErrorCode::ZeroAmount);
        require!(receiver != [0u8; 32], ErrorCode::ZeroAddress);

        let bs = &mut ctx.accounts.bridge_state;

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

        if bs.max_stake_amount != 0 {
            require!(
                actual_amount <= bs.max_stake_amount,
                ErrorCode::StakeAmountExceeded
            );
        }

        let event_amount = actual_amount.saturating_sub(bs.bridge_fee);
        require!(event_amount > 0, ErrorCode::ZeroAmount);

        let stake_record = &mut ctx.accounts.stake_record;
        stake_record.owner = ctx.accounts.user.key();
        stake_record.amount = actual_amount;
        stake_record.refunded = false;

        bs.sender_nonce = bs.sender_nonce.checked_add(1).unwrap();

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

    /// 中继器确认跨链事件。中继器身份由 Solana 原生交易签名者校验
    ///（等价于 EVM 的 msg.sender）。
    /// 使用哈希投票：每个中继器的 event_data 独立哈希并投票。
    /// 当某个哈希达到冻结的 2/3 门槛时，触发中继器的 event_data 用于解锁。
    /// 接收端扣除手续费（留在金库作为协议收入）。
    pub fn confirm_event(
        ctx: Context<ConfirmEvent>,
        _nonce: u64,
        event_data: StakeEventData,
    ) -> Result<()> {
        let bs = &mut ctx.accounts.bridge_state;

        require!(event_data.amount > 0, ErrorCode::ZeroAmount);
        require!(_nonce == event_data.nonce, ErrorCode::NonceMismatch);

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

        let req = &mut ctx.accounts.cross_chain_request;
        require!(!req.is_unlocked, ErrorCode::AlreadyProcessed);

        let relayer_key = ctx.accounts.relayer.key();
        require!(bs.is_relayer(&relayer_key), ErrorCode::RelayerNotFound);
        require!(
            !req.confirmed_relayers.contains(&relayer_key),
            ErrorCode::RelayerAlreadyConfirmed
        );

        if req.confirmed_relayers.is_empty() {
            req.nonce = event_data.nonce;
            req.frozen_threshold = ((bs.relayer_count as u16 * 2 + 2) / 3) as u8;
            req.hash_votes = Vec::new();
            req.is_unlocked = false;
        }

        req.confirmed_relayers.push(relayer_key);

        let data_bytes = event_data
            .try_to_vec()
            .map_err(|_| error!(ErrorCode::InvalidEventData))?;
        let data_hash: [u8; 32] = solana_sha256_hasher::hash(&data_bytes).to_bytes();

        let mut winning_count: u8 = 0;
        let mut vote_found = false;
        for vote in req.hash_votes.iter_mut() {
            if vote.data_hash == data_hash {
                vote.count = vote.count.checked_add(1).unwrap();
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

        if winning_count >= req.frozen_threshold && !req.is_unlocked {
            let net_amount = event_data.amount.saturating_sub(bs.bridge_fee);
            require!(net_amount > 0, ErrorCode::ZeroAmount);

            check_transfer_limits(bs, net_amount)?;

            let receiver_key = Pubkey::new_from_array(event_data.receiver);
            require!(receiver_key != Pubkey::default(), ErrorCode::ZeroAddress);
            require!(
                ctx.accounts.receiver_token_account.owner == receiver_key,
                ErrorCode::InvalidReceiver
            );

            check_vault_invariant(
                ctx.accounts.vault_token_account.amount,
                net_amount,
                bs.minimum_reserve,
            )?;

            req.is_unlocked = true;

            let vault_bump = ctx.bumps.vault;
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

    /// 操作员将某个 nonce 标记为已永久处理（接收端）。
    /// 必须在发送端退款之前调用，以防止双花。
    pub fn skip_nonce(ctx: Context<SkipNonce>, nonce: u64) -> Result<()> {
        let req = &mut ctx.accounts.cross_chain_request;
        require!(!req.is_unlocked, ErrorCode::AlreadyProcessed);
        req.is_unlocked = true;
        req.nonce = nonce;

        emit!(NonceSkipped { nonce });
        Ok(())
    }

    /// 操作员将质押金额退还给原始质押者（发送端）。
    /// 与解锁一样受速率限制和金库最低储备约束。
    pub fn refund(ctx: Context<RefundAccounts>, _nonce: u64) -> Result<()> {
        let bs = &mut ctx.accounts.bridge_state;
        let stake_record = &mut ctx.accounts.stake_record;
        let amount = stake_record.amount;

        check_transfer_limits(bs, amount)?;
        check_vault_invariant(
            ctx.accounts.vault_token_account.amount,
            amount,
            bs.minimum_reserve,
        )?;

        stake_record.refunded = true;

        let vault_bump = ctx.bumps.vault;
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

    // ─── 管理员：提取 ────────────────────────────────────────────────────

    /// 管理员从金库提取代币（受时间锁保护）。
    pub fn withdraw_token(ctx: Context<WithdrawToken>, amount: u64, to: Pubkey) -> Result<()> {
        require!(amount > 0, ErrorCode::ZeroAmount);

        let mut data = Vec::new();
        data.extend_from_slice(b"withdrawToken");
        data.extend_from_slice(&ctx.accounts.usdc_mint.key().to_bytes());
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&to.to_bytes());
        let op_hash = compute_op_hash(&data);

        consume_timelock(
            &ctx.accounts.bridge_state,
            &ctx.accounts.timelock_op,
            &op_hash,
            &ctx.accounts.admin.to_account_info(),
            ctx.program_id,
        )?;

        let vault_bump = ctx.bumps.vault;
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

    // ─── 关闭请求 ────────────────────────────────────────────────────────

    /// 关闭已完成的 CrossChainRequest PDA，将租金退还给管理员。
    pub fn close_request(ctx: Context<CloseRequest>, _nonce: u64) -> Result<()> {
        require!(
            ctx.accounts.cross_chain_request.is_unlocked,
            ErrorCode::AlreadyProcessed
        );
        Ok(())
    }
}
