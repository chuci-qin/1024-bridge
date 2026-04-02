use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar::instructions as sysvar_instructions;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

const ED25519_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("Ed25519SigVerify111111111111111111111111111");

declare_id!("7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E");

const MAX_RELAYERS: usize = 18;
const MAX_CONTRACT_LEN: usize = 64;
const MAX_FEE: u64 = 1_000_000_000;

#[program]
pub mod bridge1024 {
    use super::*;

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

    pub fn configure_usdc(ctx: Context<AdminBothStates>, usdc_mint: Pubkey) -> Result<()> {
        ctx.accounts.sender_state.usdc_mint = usdc_mint;
        ctx.accounts.receiver_state.usdc_mint = usdc_mint;
        Ok(())
    }

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

        let rs = &mut ctx.accounts.receiver_state;
        rs.source_contract = peer_contract;
        rs.source_chain_id = target_chain_id;
        rs.target_chain_id = source_chain_id;

        Ok(())
    }

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

    pub fn configure_fee(ctx: Context<AdminReceiverOnly>, fee: u64) -> Result<()> {
        require!(fee <= MAX_FEE, ErrorCode::FeeTooHigh);
        ctx.accounts.receiver_state.bridge_fee = fee;
        Ok(())
    }

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

    pub fn stake(ctx: Context<Stake>, amount: u64, receiver_address: String) -> Result<u64> {
        let ss = &mut ctx.accounts.sender_state;
        require!(!ss.is_paused, ErrorCode::Paused);
        require!(ss.usdc_mint != Pubkey::default(), ErrorCode::UsdcNotConfigured);
        require!(
            !receiver_address.is_empty() && receiver_address.len() <= 128,
            ErrorCode::InvalidReceiverAddress
        );

        let vault_balance_before = ctx.accounts.vault_token_account.amount;

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

        ctx.accounts.vault_token_account.reload()?;
        let net_amount = ctx
            .accounts
            .vault_token_account
            .amount
            .checked_sub(vault_balance_before)
            .ok_or(error!(ErrorCode::InsufficientBalance))?;

        ss.nonce += 1;
        let nonce = ss.nonce;

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
            req.nonce = event_data.nonce;
            req.event_data = event_data.clone();
            req.signed_relayers = Vec::new();
            req.is_unlocked = false;
            let threshold = ((rs.relayer_count * 2) / 3 + 1) as u8;
            req.frozen_threshold = threshold;
        } else {
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

        if req.signature_count >= req.frozen_threshold {
            req.is_unlocked = true;

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

            if rs.window_duration > 0 {
                let clock = Clock::get()?;
                let now = clock.unix_timestamp as u64;

                let window_end = rs.current_window_start.saturating_add(rs.window_duration);
                if now >= window_end {
                    let elapsed_windows =
                        (now - rs.current_window_start) / rs.window_duration;
                    rs.previous_window_usage = if elapsed_windows == 1 {
                        rs.current_window_usage
                    } else {
                        0
                    };
                    rs.current_window_start = now - (now % rs.window_duration);
                    rs.current_window_usage = 0;
                }

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

            let receiver_ta = &ctx.accounts.receiver_token_account;
            require!(
                receiver_ta.owner == event_data.receiver_address,
                ErrorCode::InvalidReceiverAddress
            );

            let vault_balance = ctx.accounts.vault_token_account.amount;
            require!(
                vault_balance >= unlock_amount.saturating_add(rs.min_reserve),
                ErrorCode::InsufficientReserve
            );

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

    pub fn propose_admin(ctx: Context<AdminBothStates>, new_admin: Pubkey) -> Result<()> {
        ctx.accounts.sender_state.pending_admin = new_admin;
        ctx.accounts.receiver_state.pending_admin = new_admin;
        Ok(())
    }

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

    pub fn pause(ctx: Context<AdminBothStates>) -> Result<()> {
        ctx.accounts.sender_state.is_paused = true;
        ctx.accounts.receiver_state.is_paused = true;
        Ok(())
    }

    pub fn unpause(ctx: Context<AdminBothStates>) -> Result<()> {
        ctx.accounts.sender_state.is_paused = false;
        ctx.accounts.receiver_state.is_paused = false;
        Ok(())
    }

    pub fn close_request(ctx: Context<CloseRequest>, _nonce: u64) -> Result<()> {
        require!(
            ctx.accounts.cross_chain_request.is_unlocked,
            ErrorCode::InvalidNonce
        );
        Ok(())
    }

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
// Account contexts
// ---------------------------------------------------------------------------

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

#[derive(Accounts)]
pub struct AdminBothStates<'info> {
    #[account(mut, seeds = [b"sender_state"], bump)]
    pub sender_state: Account<'info, SenderState>,
    #[account(mut, seeds = [b"receiver_state"], bump)]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(constraint = admin.key() == sender_state.admin @ ErrorCode::Unauthorized)]
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct AdminReceiverOnly<'info> {
    #[account(mut, seeds = [b"receiver_state"], bump)]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(constraint = admin.key() == receiver_state.admin @ ErrorCode::Unauthorized)]
    pub admin: Signer<'info>,
}

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
// State accounts
// ---------------------------------------------------------------------------

#[account]
pub struct SenderState {
    pub vault: Pubkey,
    pub admin: Pubkey,
    pub pending_admin: Pubkey,
    pub usdc_mint: Pubkey,
    pub nonce: u64,
    pub target_contract: String,
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub is_paused: bool,
}

impl SenderState {
    pub const LEN: usize =
        8 + 32 + 32 + 32 + 32 + 8 + (4 + MAX_CONTRACT_LEN) + 8 + 8 + 1;
}

#[account]
pub struct ReceiverState {
    pub vault: Pubkey,
    pub admin: Pubkey,
    pub pending_admin: Pubkey,
    pub usdc_mint: Pubkey,
    pub relayer_count: u64,
    pub source_contract: String,
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub relayers: Vec<Pubkey>,
    pub last_nonce: u64,
    pub bridge_fee: u64,
    pub is_paused: bool,
    pub max_unlock_per_window: u64,
    pub window_duration: u64,
    pub current_window_start: u64,
    pub current_window_usage: u64,
    pub previous_window_usage: u64,
    pub max_single_unlock: u64,
    pub min_reserve: u64,
}

impl ReceiverState {
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

#[account]
pub struct CrossChainRequest {
    pub nonce: u64,
    pub signed_relayers: Vec<Pubkey>,
    pub signature_count: u8,
    pub is_unlocked: bool,
    pub frozen_threshold: u8,
    pub event_data: StakeEventData,
}

impl CrossChainRequest {
    pub const LEN: usize = 8 + 8 + (4 + MAX_RELAYERS * 32) + 1 + 1 + 1 + StakeEventData::LEN;
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct StakeEventData {
    pub nonce: u64,
    pub amount: u64,
    pub block_height: u64,
    pub sender: [u8; 32],
    pub receiver_address: Pubkey,
}

impl StakeEventData {
    pub const LEN: usize = 8 + 8 + 8 + 32 + 32;
}

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
// Events
// ---------------------------------------------------------------------------

#[event]
pub struct StakeEvent {
    pub source_contract: String,
    pub target_contract: String,
    pub chain_id: u64,
    pub block_height: u64,
    pub amount: u64,
    pub sender: String,
    pub receiver_address: String,
    pub nonce: u64,
}

#[event]
pub struct CrossChainSuccessEvent {
    pub nonce: u64,
    pub amount: u64,
    pub receiver: String,
    pub source_chain_id: u64,
}

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

#[error_code]
pub enum ErrorCode {
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("USDC mint not configured")]
    UsdcNotConfigured,
    #[msg("Insufficient balance")]
    InsufficientBalance,
    #[msg("Relayer already exists")]
    RelayerAlreadyExists,
    #[msg("Relayer not found")]
    RelayerNotFound,
    #[msg("Invalid nonce")]
    InvalidNonce,
    #[msg("Invalid signature")]
    InvalidSignature,
    #[msg("Invalid source contract")]
    InvalidSourceContract,
    #[msg("Invalid chain ID")]
    InvalidChainId,
    #[msg("Too many relayers")]
    TooManyRelayers,
    #[msg("Relayer already signed")]
    RelayerAlreadySigned,
    #[msg("Invalid event data")]
    InvalidEventData,
    #[msg("Invalid receiver address")]
    InvalidReceiverAddress,
    #[msg("Bridge is paused")]
    Paused,
    #[msg("Rate limit exceeded")]
    RateLimitExceeded,
    #[msg("Single transfer limit exceeded")]
    SingleTransferExceeded,
    #[msg("Insufficient reserve")]
    InsufficientReserve,
    #[msg("Nonce mismatch")]
    NonceMismatch,
    #[msg("Already processed")]
    AlreadyProcessed,
    #[msg("Receiver token account mint mismatch")]
    ReceiverMintMismatch,
    #[msg("Fee too high")]
    FeeTooHigh,
}

// ---------------------------------------------------------------------------
// Ed25519 signature verification (SVM-H1: Wormhole-style instruction index
// validation — all three instruction indices must be 0xFFFF to prevent
// cross-instruction data injection)
// ---------------------------------------------------------------------------

fn verify_ed25519_signature(
    instructions_sysvar: &AccountInfo,
    event_data: &StakeEventData,
    signature: &[u8],
    signer_pubkey: &Pubkey,
) -> Result<()> {
    let current_index = sysvar_instructions::load_current_index_checked(instructions_sysvar)
        .map_err(|_| error!(ErrorCode::InvalidSignature))?;

    let expected_msg = event_data
        .try_to_vec()
        .map_err(|_| error!(ErrorCode::InvalidSignature))?;

    let mut found = false;

    for i in 0..current_index {
        let ix =
            sysvar_instructions::load_instruction_at_checked(i as usize, instructions_sysvar)
                .map_err(|_| error!(ErrorCode::InvalidSignature))?;

        if ix.program_id != ED25519_PROGRAM_ID {
            continue;
        }

        require!(ix.data.len() >= 16, ErrorCode::InvalidSignature);

        let num_signatures = ix.data[0];
        require!(num_signatures == 1, ErrorCode::InvalidSignature);

        let sig_offset = u16::from_le_bytes([ix.data[2], ix.data[3]]) as usize;
        let sig_ix_index = u16::from_le_bytes([ix.data[4], ix.data[5]]);
        let pk_offset = u16::from_le_bytes([ix.data[6], ix.data[7]]) as usize;
        let pk_ix_index = u16::from_le_bytes([ix.data[8], ix.data[9]]);
        let msg_offset = u16::from_le_bytes([ix.data[10], ix.data[11]]) as usize;
        let msg_size = u16::from_le_bytes([ix.data[12], ix.data[13]]) as usize;
        let msg_ix_index = u16::from_le_bytes([ix.data[14], ix.data[15]]);

        require!(sig_ix_index == 0xFFFF, ErrorCode::InvalidSignature);
        require!(pk_ix_index == 0xFFFF, ErrorCode::InvalidSignature);
        require!(msg_ix_index == 0xFFFF, ErrorCode::InvalidSignature);

        let sig_end = sig_offset
            .checked_add(64)
            .ok_or(error!(ErrorCode::InvalidSignature))?;
        require!(ix.data.len() >= sig_end, ErrorCode::InvalidSignature);
        require!(signature.len() == 64, ErrorCode::InvalidSignature);
        require!(
            ix.data[sig_offset..sig_end] == *signature,
            ErrorCode::InvalidSignature
        );

        let pk_end = pk_offset
            .checked_add(32)
            .ok_or(error!(ErrorCode::InvalidSignature))?;
        require!(ix.data.len() >= pk_end, ErrorCode::InvalidSignature);
        require!(
            ix.data[pk_offset..pk_end] == signer_pubkey.to_bytes(),
            ErrorCode::InvalidSignature
        );

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

    require!(found, ErrorCode::InvalidSignature);
    Ok(())
}
