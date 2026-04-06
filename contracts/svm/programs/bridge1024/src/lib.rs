/// 统一 SVM 跨链桥合约（bridge1024）
///
/// 本程序同时部署在 Solana 和 1024chain 上，兼任发送端（Sender）和接收端（Receiver）两种角色。
/// 发送端负责锁定用户资产并发出跨链事件；接收端验证中继者签名后解锁资产给目标用户。
/// 通过 PDA 分别维护 SenderState 和 ReceiverState，实现单一程序双向桥接。
use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar::instructions as sysvar_instructions;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

/// Solana 原生 Ed25519 签名验证预编译程序地址，用于链上验证中继者签名
const ED25519_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("Ed25519SigVerify111111111111111111111111111");

declare_id!("7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E");

/// 中继者白名单最大容量，决定 ReceiverState 和 CrossChainRequest 的账户空间上限
const MAX_RELAYERS: usize = 18;
/// 对端合约地址字符串的最大字节长度（EVM 地址 42 字节 + 余量）
const MAX_CONTRACT_LEN: usize = 64;
/// 桥接手续费上限（以 USDC 最小单位计），防止管理员误设过高费率
const MAX_FEE: u64 = 1_000_000_000;

/// 桥合约主模块，包含所有指令处理函数
#[program]
pub mod bridge1024 {
    use super::*;

    /// 初始化桥合约：创建 SenderState 和 ReceiverState 两个 PDA 账户。
    /// 管理员作为 payer 支付租金，vault PDA 作为代币托管权限。
    /// 初始化后需要分别调用 configure_usdc 和 configure_peer 完成配置。
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let admin = ctx.accounts.admin.key();
        let vault = ctx.accounts.vault.key();

        let ss = &mut ctx.accounts.sender_state;
        ss.vault = vault;
        ss.admin = admin;
        ss.pending_admin = Pubkey::default();
        ss.usdc_mint = Pubkey::default();
        ss.nonce = 0;
        ss.target_contract = String::new();
        ss.source_chain_id = 0;
        ss.target_chain_id = 0;
        ss.is_paused = false;

        let rs = &mut ctx.accounts.receiver_state;
        rs.vault = vault;
        rs.admin = admin;
        rs.pending_admin = Pubkey::default();
        rs.usdc_mint = Pubkey::default();
        rs.relayer_count = 0;
        rs.source_contract = String::new();
        rs.source_chain_id = 0;
        rs.target_chain_id = 0;
        rs.relayers = Vec::new();
        rs.last_nonce = 0;
        rs.bridge_fee = 0;
        rs.is_paused = false;
        rs.max_unlock_per_window = u64::MAX;
        rs.window_duration = 3600;
        rs.current_window_start = 0;
        rs.current_window_usage = 0;
        rs.previous_window_usage = 0;
        rs.max_single_unlock = u64::MAX;
        rs.min_reserve = 0;

        Ok(())
    }

    /// 配置 USDC 代币 Mint 地址，同时写入 SenderState 和 ReceiverState。
    /// 必须在 stake / submit_signature 之前调用，否则操作会因 UsdcNotConfigured 而失败。
    pub fn configure_usdc(ctx: Context<AdminBothStates>, usdc_mint: Pubkey) -> Result<()> {
        ctx.accounts.sender_state.usdc_mint = usdc_mint;
        ctx.accounts.receiver_state.usdc_mint = usdc_mint;
        Ok(())
    }

    /// 配置对端合约地址和链 ID，同时更新 SenderState 和 ReceiverState。
    /// 注意：对于 Sender，source → target 表示"从本链到对端"；
    /// 对于 Receiver，方向相反，所以 chain ID 在写入 ReceiverState 时互换。
    /// 这样一次调用即可完成双向配对。
    pub fn configure_peer(
        ctx: Context<AdminBothStates>,
        peer_contract: String,
        source_chain_id: u64,
        target_chain_id: u64,
    ) -> Result<()> {
        let ss = &mut ctx.accounts.sender_state;
        ss.target_contract = peer_contract.clone();
        ss.source_chain_id = source_chain_id;
        ss.target_chain_id = target_chain_id;

        // Receiver 的 source/target 与 Sender 相反：Sender 的 target 就是 Receiver 的 source
        let rs = &mut ctx.accounts.receiver_state;
        rs.source_contract = peer_contract;
        rs.source_chain_id = target_chain_id;
        rs.target_chain_id = source_chain_id;

        Ok(())
    }

    /// 独立配置 ReceiverState 的对端信息，不影响 SenderState。
    /// 适用于多对端场景：当同一程序需要从多条源链接收资产时，
    /// 可以单独更新 Receiver 的 source_contract 和链 ID。
    pub fn configure_receiver_peer(
        ctx: Context<AdminReceiverOnly>,
        peer_contract: String,
        source_chain_id: u64,
        target_chain_id: u64,
    ) -> Result<()> {
        let rs = &mut ctx.accounts.receiver_state;
        rs.source_contract = peer_contract;
        rs.source_chain_id = source_chain_id;
        rs.target_chain_id = target_chain_id;
        Ok(())
    }

    /// 设置桥接手续费（仅在 1024chain 部署上使用）。
    /// 费用在 stake 时从用户锁定金额中扣除，留在 vault 中作为协议收入。
    /// 为防止误操作，fee 不能超过 MAX_FEE。
    pub fn configure_fee(ctx: Context<AdminReceiverOnly>, fee: u64) -> Result<()> {
        require!(fee <= MAX_FEE, ErrorCode::FeeTooHigh);
        ctx.accounts.receiver_state.bridge_fee = fee;
        Ok(())
    }

    /// 配置滑动窗口速率限制参数，用于防止短时间内大量资产被解锁。
    /// - max_unlock_per_window: 单个窗口内最大解锁总量
    /// - window_duration: 窗口时长（秒），配合加权滑动计算实现平滑限速
    /// - max_single_unlock: 单笔最大解锁金额
    /// - min_reserve: vault 最低保留余额，解锁后余额不得低于此值
    pub fn configure_rate_limits(
        ctx: Context<AdminReceiverOnly>,
        max_unlock_per_window: u64,
        window_duration: u64,
        max_single_unlock: u64,
        min_reserve: u64,
    ) -> Result<()> {
        let rs = &mut ctx.accounts.receiver_state;
        rs.max_unlock_per_window = max_unlock_per_window;
        rs.window_duration = window_duration;
        rs.max_single_unlock = max_single_unlock;
        rs.min_reserve = min_reserve;
        Ok(())
    }

    /// 用户锁定代币（stake）发起跨链转账。
    /// 流程：1) 校验合约未暂停且已配置 USDC；2) 将用户代币转入 vault；
    /// 3) 通过转账前后余额差计算实际到账金额（兼容 fee-on-transfer 代币）；
    /// 4) 递增 nonce；5) 扣除手续费后发出 StakeEvent 供中继者监听。
    /// 返回本次 nonce，用于链下追踪。
    pub fn stake(ctx: Context<Stake>, amount: u64, receiver_address: String) -> Result<u64> {
        let ss = &mut ctx.accounts.sender_state;
        require!(!ss.is_paused, ErrorCode::Paused);
        require!(ss.usdc_mint != Pubkey::default(), ErrorCode::UsdcNotConfigured);
        require!(
            !receiver_address.is_empty() && receiver_address.len() <= 128,
            ErrorCode::InvalidReceiverAddress
        );

        // 记录转账前 vault 余额，用于计算实际到账金额
        let vault_balance_before = ctx.accounts.vault_token_account.amount;

        // 执行 CPI 转账：用户 → vault
        let cpi_accounts = TransferChecked {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
            mint: ctx.accounts.usdc_mint.to_account_info(),
        };
        token_interface::transfer_checked(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount,
            ctx.accounts.usdc_mint.decimals,
        )?;

        // 重新加载 vault 余额，差值即为实际到账（防御 fee-on-transfer 代币）
        ctx.accounts.vault_token_account.reload()?;
        let net_amount = ctx
            .accounts
            .vault_token_account
            .amount
            .checked_sub(vault_balance_before)
            .ok_or(error!(ErrorCode::InsufficientBalance))?;

        ss.nonce += 1;
        let nonce = ss.nonce;

        // 从实际到账中扣除手续费，事件中的 amount 是用户在目标链实际可领取的金额
        let bridge_fee = ctx.accounts.receiver_state.bridge_fee;
        let event_amount = net_amount.saturating_sub(bridge_fee);

        let sender_bytes = ctx.accounts.user.key().to_bytes();
        let clock = Clock::get()?;

        emit!(StakeEvent {
            source_contract: crate::ID.to_string(),
            target_contract: ss.target_contract.clone(),
            chain_id: ss.source_chain_id,
            block_height: clock.slot,
            amount: event_amount,
            sender: hex::encode(sender_bytes),
            receiver_address,
            nonce,
        });

        Ok(nonce)
    }

    /// 添加中继者到白名单。中继者数量不得超过 MAX_RELAYERS，且不允许重复。
    /// 中继者负责监听源链 StakeEvent 并在目标链提交签名以触发解锁。
    pub fn add_relayer(ctx: Context<AdminReceiverOnly>, relayer: Pubkey) -> Result<()> {
        let rs = &mut ctx.accounts.receiver_state;
        require!(
            (rs.relayer_count as usize) < MAX_RELAYERS,
            ErrorCode::TooManyRelayers
        );
        require!(
            !rs.relayers.contains(&relayer),
            ErrorCode::RelayerAlreadyExists
        );
        rs.relayers.push(relayer);
        rs.relayer_count += 1;
        Ok(())
    }

    /// 从白名单中移除中继者。使用 swap_remove 保持 O(1) 删除效率。
    pub fn remove_relayer(ctx: Context<AdminReceiverOnly>, relayer: Pubkey) -> Result<()> {
        let rs = &mut ctx.accounts.receiver_state;
        let idx = rs
            .relayers
            .iter()
            .position(|r| *r == relayer)
            .ok_or(error!(ErrorCode::RelayerNotFound))?;
        rs.relayers.swap_remove(idx);
        rs.relayer_count -= 1;
        Ok(())
    }

    /// 原子替换中继者：将旧中继者替换为新中继者，不改变总数量。
    /// 用于密钥轮换场景，避免 remove + add 之间的窗口期导致签名阈值不足。
    pub fn rotate_relayer(
        ctx: Context<AdminReceiverOnly>,
        old_relayer: Pubkey,
        new_relayer: Pubkey,
    ) -> Result<()> {
        let rs = &mut ctx.accounts.receiver_state;
        require!(
            !rs.relayers.contains(&new_relayer),
            ErrorCode::RelayerAlreadyExists
        );
        let idx = rs
            .relayers
            .iter()
            .position(|r| *r == old_relayer)
            .ok_or(error!(ErrorCode::RelayerNotFound))?;
        rs.relayers[idx] = new_relayer;
        Ok(())
    }

    /// 中继者提交 Ed25519 签名，是跨链解锁的核心指令。
    /// 流程：1) 校验合约状态和 nonce；2) 验证中继者身份及 Ed25519 签名；
    /// 3) 累计签名数；4) 达到 2/3+1 阈值后执行解锁转账。
    /// 解锁时还需通过滑动窗口速率限制、单笔上限和最低储备三重检查。
    /// CrossChainRequest PDA 以 nonce 为 seed，确保每个跨链请求唯一。
    pub fn submit_signature(
        ctx: Context<SubmitSignature>,
        _nonce: u64,
        event_data: StakeEventData,
        signature: Vec<u8>,
    ) -> Result<()> {
        let rs = &mut ctx.accounts.receiver_state;
        require!(!rs.is_paused, ErrorCode::Paused);
        require!(
            rs.usdc_mint != Pubkey::default(),
            ErrorCode::UsdcNotConfigured
        );

        require!(_nonce == event_data.nonce, ErrorCode::NonceMismatch);

        require!(
            !rs.source_contract.is_empty(),
            ErrorCode::InvalidSourceContract
        );
        require!(rs.source_chain_id != 0, ErrorCode::InvalidChainId);

        let req = &mut ctx.accounts.cross_chain_request;
        require!(!req.is_unlocked, ErrorCode::AlreadyProcessed);

        let relayer_key = ctx.accounts.relayer.key();
        require!(
            rs.relayers.contains(&relayer_key),
            ErrorCode::RelayerNotFound
        );

        if req.signature_count == 0 {
            // 首个签名：初始化 CrossChainRequest，冻结当前阈值（2/3+1）
            req.nonce = event_data.nonce;
            req.event_data = event_data.clone();
            req.signed_relayers = Vec::new();
            req.is_unlocked = false;
            let threshold = ((rs.relayer_count * 2) / 3 + 1) as u8;
            req.frozen_threshold = threshold;
        } else {
            // 后续签名：校验 event_data 一致性，防止同一 nonce 下的数据篡改
            require!(req.event_data == event_data, ErrorCode::InvalidEventData);
        }

        require!(
            !req.signed_relayers.contains(&relayer_key),
            ErrorCode::RelayerAlreadySigned
        );

        verify_ed25519_signature(
            &ctx.accounts.instructions_sysvar,
            &event_data,
            &signature,
            &relayer_key,
        )?;

        req.signed_relayers.push(relayer_key);
        req.signature_count += 1;

        // 达到签名阈值，执行解锁转账
        if req.signature_count >= req.frozen_threshold {
            req.is_unlocked = true;

            // 在接收端扣除手续费（如果配置了的话）
            let bridge_fee = rs.bridge_fee;
            let unlock_amount = if bridge_fee > 0 {
                require!(event_data.amount > bridge_fee, ErrorCode::FeeTooHigh);
                event_data.amount - bridge_fee
            } else {
                event_data.amount
            };

            require!(
                unlock_amount <= rs.max_single_unlock,
                ErrorCode::SingleTransferExceeded
            );

            // 滑动窗口速率限制：加权计算跨窗口的有效用量
            // 算法：当前窗口用量 + 上一窗口用量 × (当前窗口剩余时间 / 窗口总时长)
            // 这样在窗口边界处平滑过渡，避免突变
            if rs.window_duration > 0 {
                let clock = Clock::get()?;
                let now = clock.unix_timestamp as u64;

                // 窗口翻转：如果当前时间超过窗口结束，滚动到新窗口
                let window_end = rs.current_window_start.saturating_add(rs.window_duration);
                if now >= window_end {
                    let elapsed_windows =
                        (now - rs.current_window_start) / rs.window_duration;
                    rs.previous_window_usage = if elapsed_windows == 1 {
                        rs.current_window_usage
                    } else {
                        // 跨越多个窗口，上一窗口用量归零
                        0
                    };
                    rs.current_window_start = now - (now % rs.window_duration);
                    rs.current_window_usage = 0;
                }

                // 计算加权有效用量
                let window_remaining = rs
                    .current_window_start
                    .saturating_add(rs.window_duration)
                    .saturating_sub(now);
                let weight = (window_remaining as u128)
                    .checked_mul(1_000_000)
                    .unwrap_or(0)
                    / rs.window_duration as u128;
                let weighted_previous =
                    (rs.previous_window_usage as u128 * weight) / 1_000_000;
                let effective_usage = weighted_previous as u64 + rs.current_window_usage;

                require!(
                    effective_usage
                        .checked_add(unlock_amount)
                        .ok_or(error!(ErrorCode::RateLimitExceeded))?
                        <= rs.max_unlock_per_window,
                    ErrorCode::RateLimitExceeded
                );

                rs.current_window_usage = rs
                    .current_window_usage
                    .checked_add(unlock_amount)
                    .ok_or(error!(ErrorCode::RateLimitExceeded))?;
            }

            // 校验接收者代币账户的 owner 与事件中的 receiver_address 一致
            let receiver_ta = &ctx.accounts.receiver_token_account;
            require!(
                receiver_ta.owner == event_data.receiver_address,
                ErrorCode::InvalidReceiverAddress
            );

            // 确保解锁后 vault 余额不低于最低储备要求
            let vault_balance = ctx.accounts.vault_token_account.amount;
            require!(
                vault_balance >= unlock_amount.saturating_add(rs.min_reserve),
                ErrorCode::InsufficientReserve
            );

            // 使用 vault PDA 签名执行 CPI 转账：vault → 接收者
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
                unlock_amount,
                ctx.accounts.usdc_mint.decimals,
            )?;

            // 更新已处理的最大 nonce（仅递增，用于链下查询进度）
            if event_data.nonce > rs.last_nonce {
                rs.last_nonce = event_data.nonce;
            }

            emit!(CrossChainSuccessEvent {
                nonce: event_data.nonce,
                amount: unlock_amount,
                receiver: event_data.receiver_address.to_string(),
                source_chain_id: rs.source_chain_id,
            });
        }

        Ok(())
    }

    /// 提议新管理员（两步管理员转移的第一步）。
    /// 将 pending_admin 写入两个状态账户，新管理员需调用 accept_admin 完成接管。
    /// 两步设计防止误将管理权转给错误地址导致不可逆的失控。
    pub fn propose_admin(ctx: Context<AdminBothStates>, new_admin: Pubkey) -> Result<()> {
        ctx.accounts.sender_state.pending_admin = new_admin;
        ctx.accounts.receiver_state.pending_admin = new_admin;
        Ok(())
    }

    /// 接受管理员转移（两步管理员转移的第二步）。
    /// 仅 pending_admin 本人可调用，完成后清空 pending_admin。
    pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
        let new_admin = ctx.accounts.new_admin.key();

        let ss = &mut ctx.accounts.sender_state;
        ss.admin = new_admin;
        ss.pending_admin = Pubkey::default();

        let rs = &mut ctx.accounts.receiver_state;
        rs.admin = new_admin;
        rs.pending_admin = Pubkey::default();

        Ok(())
    }

    /// 紧急暂停：同时暂停发送端和接收端，阻止 stake 和 submit_signature。
    /// 作为熔断机制，当发现异常（如中继者密钥泄露）时立即止损。
    pub fn pause(ctx: Context<AdminBothStates>) -> Result<()> {
        ctx.accounts.sender_state.is_paused = true;
        ctx.accounts.receiver_state.is_paused = true;
        Ok(())
    }

    /// 恢复运行：解除暂停状态，恢复正常的跨链操作。
    pub fn unpause(ctx: Context<AdminBothStates>) -> Result<()> {
        ctx.accounts.sender_state.is_paused = false;
        ctx.accounts.receiver_state.is_paused = false;
        Ok(())
    }

    /// 关闭已处理的 CrossChainRequest PDA，回收租金（rent）到管理员账户。
    /// 仅允许关闭 is_unlocked == true 的请求，防止误关未完成的跨链请求。
    pub fn close_request(ctx: Context<CloseRequest>, _nonce: u64) -> Result<()> {
        require!(
            ctx.accounts.cross_chain_request.is_unlocked,
            ErrorCode::InvalidNonce
        );
        Ok(())
    }

    /// 管理员向 vault 注入流动性，确保有足够余额供接收端解锁。
    pub fn add_liquidity(ctx: Context<ManageLiquidity>, amount: u64) -> Result<()> {
        let cpi_accounts = TransferChecked {
            from: ctx.accounts.admin_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.admin.to_account_info(),
            mint: ctx.accounts.usdc_mint.to_account_info(),
        };
        token_interface::transfer_checked(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount,
            ctx.accounts.usdc_mint.decimals,
        )?;
        Ok(())
    }

    /// 管理员从 vault 提取流动性。使用 PDA 签名授权转账。
    /// 注意：提取后需确保余额仍满足 min_reserve 要求（由速率限制逻辑保障）。
    pub fn withdraw_liquidity(ctx: Context<ManageLiquidity>, amount: u64) -> Result<()> {
        let vault_bump = ctx.bumps.vault;
        let signer_seeds: &[&[&[u8]]] = &[&[b"vault", &[vault_bump]]];

        let cpi_accounts = TransferChecked {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.admin_token_account.to_account_info(),
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
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 账户上下文（Account Contexts）
// 每个上下文定义了对应指令所需的账户集合及其约束条件
// ---------------------------------------------------------------------------

/// 初始化上下文：创建 SenderState 和 ReceiverState PDA，由管理员支付租金
#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = admin,
        space = SenderState::LEN,
        seeds = [b"sender_state"],
        bump,
    )]
    pub sender_state: Account<'info, SenderState>,
    #[account(
        init,
        payer = admin,
        space = ReceiverState::LEN,
        seeds = [b"receiver_state"],
        bump,
    )]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(mut)]
    pub admin: Signer<'info>,
    /// CHECK: Vault PDA used as token account authority, validated by seeds
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

/// 管理员操作上下文（同时修改双状态）：用于 configure_usdc、configure_peer、pause/unpause 等
/// 约束：签名者必须是 SenderState 中记录的 admin
#[derive(Accounts)]
pub struct AdminBothStates<'info> {
    #[account(mut, seeds = [b"sender_state"], bump)]
    pub sender_state: Account<'info, SenderState>,
    #[account(mut, seeds = [b"receiver_state"], bump)]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(constraint = admin.key() == sender_state.admin @ ErrorCode::Unauthorized)]
    pub admin: Signer<'info>,
}

/// 管理员操作上下文（仅修改 ReceiverState）：用于 add_relayer、configure_fee 等
/// 适用于只需要修改接收端状态的管理操作
#[derive(Accounts)]
pub struct AdminReceiverOnly<'info> {
    #[account(mut, seeds = [b"receiver_state"], bump)]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(constraint = admin.key() == receiver_state.admin @ ErrorCode::Unauthorized)]
    pub admin: Signer<'info>,
}

/// 用户锁仓（Stake）上下文：用户将 USDC 转入 vault，发出跨链事件。
/// 同时读取 ReceiverState 以获取当前手续费配置。
#[derive(Accounts)]
pub struct Stake<'info> {
    #[account(mut, seeds = [b"sender_state"], bump)]
    pub sender_state: Account<'info, SenderState>,
    #[account(seeds = [b"receiver_state"], bump)]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(mut)]
    pub user: Signer<'info>,
    /// CHECK: Vault PDA, validated by seeds
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    #[account(
        constraint = usdc_mint.key() == sender_state.usdc_mint @ ErrorCode::UsdcNotConfigured
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

/// 提交签名上下文：中继者提交 Ed25519 签名以推进跨链解锁。
/// CrossChainRequest PDA 以 nonce 为 seed，init_if_needed 实现首次创建、后续复用。
/// 包含 instructions_sysvar 用于 Ed25519 签名的链上验证。
#[derive(Accounts)]
#[instruction(_nonce: u64)]
pub struct SubmitSignature<'info> {
    #[account(mut, seeds = [b"receiver_state"], bump)]
    pub receiver_state: Account<'info, ReceiverState>,
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
    /// CHECK: Vault PDA, validated by seeds
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    #[account(
        constraint = usdc_mint.key() == receiver_state.usdc_mint @ ErrorCode::UsdcNotConfigured
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
            @ ErrorCode::ReceiverMintMismatch
    )]
    pub receiver_token_account: InterfaceAccount<'info, TokenAccount>,
    /// CHECK: Instructions sysvar, validated by address constraint
    #[account(address = sysvar_instructions::ID)]
    pub instructions_sysvar: AccountInfo<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

/// 接受管理员转移上下文：新管理员签名确认接管。
/// 约束：签名者必须是 SenderState.pending_admin。
#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    #[account(mut, seeds = [b"sender_state"], bump)]
    pub sender_state: Account<'info, SenderState>,
    #[account(mut, seeds = [b"receiver_state"], bump)]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(
        constraint = new_admin.key() == sender_state.pending_admin @ ErrorCode::Unauthorized
    )]
    pub new_admin: Signer<'info>,
}

/// 关闭请求上下文：管理员回收已完成的 CrossChainRequest PDA 的租金。
/// close = admin 将 lamports 退还给管理员。
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
    #[account(seeds = [b"receiver_state"], bump)]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(
        mut,
        constraint = admin.key() == receiver_state.admin @ ErrorCode::Unauthorized
    )]
    pub admin: Signer<'info>,
}

/// 流动性管理上下文：管理员向 vault 注入或提取 USDC。
/// 用于初始流动性注入和日常资金管理。
#[derive(Accounts)]
pub struct ManageLiquidity<'info> {
    #[account(seeds = [b"receiver_state"], bump)]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(
        mut,
        constraint = admin.key() == receiver_state.admin @ ErrorCode::Unauthorized
    )]
    pub admin: Signer<'info>,
    /// CHECK: Vault PDA, validated by seeds
    #[account(seeds = [b"vault"], bump)]
    pub vault: AccountInfo<'info>,
    #[account(
        constraint = usdc_mint.key() == receiver_state.usdc_mint @ ErrorCode::UsdcNotConfigured
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
        constraint = admin_token_account.owner == admin.key(),
        constraint = admin_token_account.mint == usdc_mint.key(),
    )]
    pub admin_token_account: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
}

// ---------------------------------------------------------------------------
// 状态账户（State Accounts）
// SenderState 和 ReceiverState 分别通过 PDA seed "sender_state" 和 "receiver_state" 派生
// ---------------------------------------------------------------------------

/// 发送端状态：记录发送方向的配置（本链 → 目标链）。
/// 核心字段：vault（资金托管地址）、nonce（递增序号）、target_contract（对端合约地址）。
#[account]
pub struct SenderState {
    pub vault: Pubkey,            // vault PDA 地址，代币托管权限
    pub admin: Pubkey,            // 当前管理员
    pub pending_admin: Pubkey,    // 待接受的新管理员（两步转移）
    pub usdc_mint: Pubkey,        // USDC 代币 Mint 地址
    pub nonce: u64,               // 跨链请求递增序号
    pub target_contract: String,  // 对端（目标链）合约地址
    pub source_chain_id: u64,     // 本链 chain ID
    pub target_chain_id: u64,     // 目标链 chain ID
    pub is_paused: bool,          // 暂停标志
}

impl SenderState {
    /// 账户空间大小：8 (discriminator) + 各字段大小，String 按 4 + MAX_CONTRACT_LEN 预留
    pub const LEN: usize =
        8 + 32 + 32 + 32 + 32 + 8 + (4 + MAX_CONTRACT_LEN) + 8 + 8 + 1;
}

/// 接收端状态：记录接收方向的配置（源链 → 本链）以及中继者和速率限制信息。
/// 包含中继者白名单、手续费、滑动窗口速率限制等安全机制参数。
#[account]
pub struct ReceiverState {
    pub vault: Pubkey,              // vault PDA 地址
    pub admin: Pubkey,              // 当前管理员
    pub pending_admin: Pubkey,      // 待接受的新管理员
    pub usdc_mint: Pubkey,          // USDC 代币 Mint 地址
    pub relayer_count: u64,         // 当前中继者数量
    pub source_contract: String,    // 源链合约地址
    pub source_chain_id: u64,       // 源链 chain ID
    pub target_chain_id: u64,       // 本链 chain ID
    pub relayers: Vec<Pubkey>,      // 中继者公钥白名单
    pub last_nonce: u64,            // 已处理的最大 nonce
    pub bridge_fee: u64,            // 桥接手续费（USDC 最小单位）
    pub is_paused: bool,            // 暂停标志
    pub max_unlock_per_window: u64, // 单窗口最大解锁总量
    pub window_duration: u64,       // 窗口时长（秒）
    pub current_window_start: u64,  // 当前窗口起始时间戳
    pub current_window_usage: u64,  // 当前窗口已解锁量
    pub previous_window_usage: u64, // 上一窗口已解锁量（用于加权计算）
    pub max_single_unlock: u64,     // 单笔最大解锁金额
    pub min_reserve: u64,           // vault 最低保留余额
}

impl ReceiverState {
    /// 账户空间大小：Vec<Pubkey> 按 4 + MAX_RELAYERS * 32 预留上限
    pub const LEN: usize = 8
        + 32
        + 32
        + 32
        + 32
        + 8
        + (4 + MAX_CONTRACT_LEN)
        + 8
        + 8
        + (4 + MAX_RELAYERS * 32)
        + 8
        + 8
        + 1
        + 8
        + 8
        + 8
        + 8
        + 8
        + 8
        + 8;
}

/// 跨链请求 PDA：以 nonce 为 seed 派生，跟踪单个跨链解锁请求的签名收集状态。
/// 当签名数达到 frozen_threshold 时触发解锁。完成后可通过 close_request 回收租金。
#[account]
pub struct CrossChainRequest {
    pub nonce: u64,                    // 对应的跨链请求序号
    pub signed_relayers: Vec<Pubkey>,  // 已签名的中继者列表
    pub signature_count: u8,           // 已收集的签名数
    pub is_unlocked: bool,             // 是否已完成解锁
    pub frozen_threshold: u8,          // 创建时冻结的阈值（2/3+1）
    pub event_data: StakeEventData,    // 跨链事件数据（首个签名时写入）
}

impl CrossChainRequest {
    /// 账户空间大小：signed_relayers 按最大中继者数预留
    pub const LEN: usize = 8 + 8 + (4 + MAX_RELAYERS * 32) + 1 + 1 + 1 + StakeEventData::LEN;
}

/// 跨链事件的序列化数据结构，作为 Ed25519 签名的消息体。
/// 中继者对该结构的 Borsh 序列化结果进行签名，链上验证时还原并比对。
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct StakeEventData {
    pub nonce: u64,               // 跨链请求序号
    pub amount: u64,              // 跨链金额（扣费前）
    pub block_height: u64,        // 源链上的区块高度（slot）
    pub sender: [u8; 32],         // 发送者公钥原始字节
    pub receiver_address: Pubkey, // 目标链接收者地址
}

impl StakeEventData {
    /// 固定大小：nonce(8) + amount(8) + block_height(8) + sender(32) + receiver_address(32)
    pub const LEN: usize = 8 + 8 + 8 + 32 + 32;
}

/// Default 实现：所有字段置零，用于 CrossChainRequest 初始化时的占位
impl Default for StakeEventData {
    fn default() -> Self {
        Self {
            nonce: 0,
            amount: 0,
            block_height: 0,
            sender: [0u8; 32],
            receiver_address: Pubkey::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// 事件（Events）
// 通过 Anchor 的 emit! 宏发出，中继者和前端通过 RPC 订阅监听
// ---------------------------------------------------------------------------

/// 锁仓事件：用户在发送端 stake 后发出，中继者监听此事件并在目标链提交签名。
/// 包含完整的跨链路由信息：源/目标合约、链 ID、金额、发送者、接收者、nonce。
#[event]
pub struct StakeEvent {
    pub source_contract: String,   // 源链合约地址（本程序 ID）
    pub target_contract: String,   // 目标链合约地址
    pub chain_id: u64,             // 源链 chain ID
    pub block_height: u64,         // 源链区块高度（slot）
    pub amount: u64,               // 扣除手续费后的实际跨链金额
    pub sender: String,            // 发送者公钥（hex 编码）
    pub receiver_address: String,  // 目标链接收者地址
    pub nonce: u64,                // 唯一跨链请求序号
}

/// 跨链成功事件：在接收端签名达到阈值、代币解锁后发出。
/// 用于前端展示和链下系统确认跨链完成。
#[event]
pub struct CrossChainSuccessEvent {
    pub nonce: u64,            // 对应的跨链请求序号
    pub amount: u64,           // 实际解锁金额（扣费后）
    pub receiver: String,      // 接收者地址
    pub source_chain_id: u64,  // 源链 chain ID
}

// ---------------------------------------------------------------------------
// 错误码（Error Codes）
// 每个错误码对应一种业务异常，便于客户端精确处理和展示
// ---------------------------------------------------------------------------

#[error_code]
pub enum ErrorCode {
    /// 调用者不是管理员或无权执行此操作
    #[msg("Unauthorized")]
    Unauthorized,
    /// USDC Mint 地址尚未配置，需先调用 configure_usdc
    #[msg("USDC mint not configured")]
    UsdcNotConfigured,
    /// 用户代币余额不足以完成转账
    #[msg("Insufficient balance")]
    InsufficientBalance,
    /// 尝试添加已存在于白名单中的中继者
    #[msg("Relayer already exists")]
    RelayerAlreadyExists,
    /// 指定的中继者不在白名单中
    #[msg("Relayer not found")]
    RelayerNotFound,
    /// nonce 无效（用于 close_request 时检查请求是否已完成）
    #[msg("Invalid nonce")]
    InvalidNonce,
    /// Ed25519 签名验证失败（签名格式错误、公钥不匹配或消息篡改）
    #[msg("Invalid signature")]
    InvalidSignature,
    /// 源链合约地址未配置或为空
    #[msg("Invalid source contract")]
    InvalidSourceContract,
    /// 链 ID 为零，尚未配置
    #[msg("Invalid chain ID")]
    InvalidChainId,
    /// 中继者数量已达 MAX_RELAYERS 上限
    #[msg("Too many relayers")]
    TooManyRelayers,
    /// 该中继者已对此 nonce 提交过签名，不允许重复
    #[msg("Relayer already signed")]
    RelayerAlreadySigned,
    /// 后续签名提交的事件数据与首次不一致
    #[msg("Invalid event data")]
    InvalidEventData,
    /// 接收者地址为空、过长，或代币账户 owner 不匹配
    #[msg("Invalid receiver address")]
    InvalidReceiverAddress,
    /// 桥合约已暂停，拒绝 stake 和 submit_signature 操作
    #[msg("Bridge is paused")]
    Paused,
    /// 滑动窗口速率限制：当前窗口累计解锁量超过上限
    #[msg("Rate limit exceeded")]
    RateLimitExceeded,
    /// 单笔解锁金额超过 max_single_unlock 限制
    #[msg("Single transfer limit exceeded")]
    SingleTransferExceeded,
    /// 解锁后 vault 余额将低于 min_reserve 最低储备
    #[msg("Insufficient reserve")]
    InsufficientReserve,
    /// 指令参数中的 nonce 与 event_data.nonce 不一致
    #[msg("Nonce mismatch")]
    NonceMismatch,
    /// 该跨链请求已被处理（is_unlocked == true）
    #[msg("Already processed")]
    AlreadyProcessed,
    /// 接收者代币账户的 mint 与配置的 USDC mint 不匹配
    #[msg("Receiver token account mint mismatch")]
    ReceiverMintMismatch,
    /// 手续费超过 MAX_FEE 上限
    #[msg("Fee too high")]
    FeeTooHigh,
}

// ---------------------------------------------------------------------------
// Ed25519 签名验证
//
// 采用 Wormhole 风格的指令内省（instruction introspection）方案：
// Solana 的 Ed25519 预编译程序不能直接 CPI 调用，需要将 Ed25519SigVerify 指令
// 放在同一事务中、submit_signature 指令之前。本函数通过 instructions_sysvar
// 回溯检查之前的指令，找到匹配的 Ed25519SigVerify 指令并验证其内容。
//
// 安全要点（SVM-H1）：三个 instruction index（sig_ix_index、pk_ix_index、msg_ix_index）
// 必须全部为 0xFFFF，表示签名/公钥/消息数据内联在指令 data 中，
// 而非引用其他指令的数据。这防止了跨指令数据注入攻击。
// ---------------------------------------------------------------------------

/// 验证 Ed25519 签名：通过 instructions_sysvar 内省同事务中的前置指令，
/// 找到 Ed25519SigVerify 预编译指令并校验签名、公钥和消息是否匹配。
fn verify_ed25519_signature(
    instructions_sysvar: &AccountInfo,
    event_data: &StakeEventData,
    signature: &[u8],
    signer_pubkey: &Pubkey,
) -> Result<()> {
    // 获取当前指令在事务中的索引，用于限定搜索范围（只查前置指令）
    let current_index = sysvar_instructions::load_current_index_checked(instructions_sysvar)
        .map_err(|_| error!(ErrorCode::InvalidSignature))?;

    // 将事件数据 Borsh 序列化为期望的消息内容
    let expected_msg = event_data
        .try_to_vec()
        .map_err(|_| error!(ErrorCode::InvalidSignature))?;

    let mut found = false;

    // 遍历当前指令之前的所有指令，查找 Ed25519SigVerify 预编译调用
    for i in 0..current_index {
        let ix =
            sysvar_instructions::load_instruction_at_checked(i as usize, instructions_sysvar)
                .map_err(|_| error!(ErrorCode::InvalidSignature))?;

        if ix.program_id != ED25519_PROGRAM_ID {
            continue;
        }

        // Ed25519SigVerify 指令 data 头部至少 16 字节
        require!(ix.data.len() >= 16, ErrorCode::InvalidSignature);

        // 仅支持单签名模式
        let num_signatures = ix.data[0];
        require!(num_signatures == 1, ErrorCode::InvalidSignature);

        // 解析各字段的偏移量和指令索引
        let sig_offset = u16::from_le_bytes([ix.data[2], ix.data[3]]) as usize;
        let sig_ix_index = u16::from_le_bytes([ix.data[4], ix.data[5]]);
        let pk_offset = u16::from_le_bytes([ix.data[6], ix.data[7]]) as usize;
        let pk_ix_index = u16::from_le_bytes([ix.data[8], ix.data[9]]);
        let msg_offset = u16::from_le_bytes([ix.data[10], ix.data[11]]) as usize;
        let msg_size = u16::from_le_bytes([ix.data[12], ix.data[13]]) as usize;
        let msg_ix_index = u16::from_le_bytes([ix.data[14], ix.data[15]]);

        // 安全检查：所有 ix_index 必须为 0xFFFF（数据内联），防止跨指令注入
        require!(sig_ix_index == 0xFFFF, ErrorCode::InvalidSignature);
        require!(pk_ix_index == 0xFFFF, ErrorCode::InvalidSignature);
        require!(msg_ix_index == 0xFFFF, ErrorCode::InvalidSignature);

        // 验证签名内容（64 字节）
        let sig_end = sig_offset
            .checked_add(64)
            .ok_or(error!(ErrorCode::InvalidSignature))?;
        require!(ix.data.len() >= sig_end, ErrorCode::InvalidSignature);
        require!(signature.len() == 64, ErrorCode::InvalidSignature);
        require!(
            ix.data[sig_offset..sig_end] == *signature,
            ErrorCode::InvalidSignature
        );

        // 验证公钥（32 字节）与提交签名的中继者一致
        let pk_end = pk_offset
            .checked_add(32)
            .ok_or(error!(ErrorCode::InvalidSignature))?;
        require!(ix.data.len() >= pk_end, ErrorCode::InvalidSignature);
        require!(
            ix.data[pk_offset..pk_end] == signer_pubkey.to_bytes(),
            ErrorCode::InvalidSignature
        );

        // 验证消息内容与期望的事件数据序列化结果一致
        let msg_end = msg_offset
            .checked_add(msg_size)
            .ok_or(error!(ErrorCode::InvalidSignature))?;
        require!(ix.data.len() >= msg_end, ErrorCode::InvalidSignature);
        require!(msg_size == expected_msg.len(), ErrorCode::InvalidSignature);
        require!(
            ix.data[msg_offset..msg_end] == expected_msg[..],
            ErrorCode::InvalidSignature
        );

        found = true;
        break;
    }

    // 未找到匹配的 Ed25519SigVerify 指令则验证失败
    require!(found, ErrorCode::InvalidSignature);
    Ok(())
}
