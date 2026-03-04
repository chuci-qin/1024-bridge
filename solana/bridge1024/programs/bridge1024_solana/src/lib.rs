use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("DtB1mvEcpWQdDxcmQPXjoe5dsrugBfU7NZjsLQwQ3KH5");

#[program]
pub mod bridge1024_solana {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let sender_state = &mut ctx.accounts.sender_state;
        let receiver_state = &mut ctx.accounts.receiver_state;

        sender_state.vault = ctx.accounts.vault.key();
        sender_state.admin = ctx.accounts.admin.key();
        sender_state.nonce = 0;
        sender_state.usdc_mint = Pubkey::default();
        sender_state.target_contract = String::new();
        sender_state.source_chain_id = 0;
        sender_state.target_chain_id = 0;

        receiver_state.vault = ctx.accounts.vault.key();
        receiver_state.admin = ctx.accounts.admin.key();
        receiver_state.last_nonce = 0;
        receiver_state.relayer_count = 0;
        receiver_state.usdc_mint = Pubkey::default();
        receiver_state.source_contract = String::new();
        receiver_state.source_chain_id = 0;
        receiver_state.target_chain_id = 0;
        receiver_state.relayers = Vec::new();

        Ok(())
    }

    pub fn configure_usdc(ctx: Context<ConfigureUsdc>, usdc_mint: Pubkey) -> Result<()> {
        ctx.accounts.sender_state.usdc_mint = usdc_mint;
        ctx.accounts.receiver_state.usdc_mint = usdc_mint;
        Ok(())
    }

    pub fn configure_peer(
        ctx: Context<ConfigurePeer>,
        peer_contract: Pubkey,
        source_chain_id: u64,
        target_chain_id: u64,
    ) -> Result<()> {
        let sender_state = &mut ctx.accounts.sender_state;
        let receiver_state = &mut ctx.accounts.receiver_state;

        sender_state.target_contract = peer_contract.to_string();
        sender_state.source_chain_id = source_chain_id;
        sender_state.target_chain_id = target_chain_id;

        receiver_state.source_contract = peer_contract.to_string();
        receiver_state.source_chain_id = target_chain_id;
        receiver_state.target_chain_id = source_chain_id;

        Ok(())
    }

    pub fn configure_receiver_peer(
        ctx: Context<ConfigureReceiverPeer>,
        peer_contract: String,
        source_chain_id: u64,
        target_chain_id: u64,
    ) -> Result<()> {
        let receiver_state = &mut ctx.accounts.receiver_state;
        receiver_state.source_contract = peer_contract;
        receiver_state.source_chain_id = source_chain_id;
        receiver_state.target_chain_id = target_chain_id;
        Ok(())
    }

    pub fn stake(ctx: Context<Stake>, amount: u64, receiver_address: String) -> Result<u64> {
        let sender_state = &mut ctx.accounts.sender_state;

        require!(
            sender_state.usdc_mint != Pubkey::default(),
            ErrorCode::UsdcNotConfigured
        );

        let cpi_accounts = TransferChecked {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
            mint: ctx.accounts.usdc_mint.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token_interface::transfer_checked(cpi_ctx, amount, ctx.accounts.usdc_mint.decimals)?;

        let current_nonce = sender_state.nonce;
        let new_nonce = current_nonce.wrapping_add(1);
        if new_nonce == 0 && current_nonce != u64::MAX {
            return Err(ErrorCode::InvalidNonce.into());
        }
        sender_state.nonce = new_nonce;

        emit!(StakeEvent {
            source_contract: ctx.program_id.to_string(),
            target_contract: sender_state.target_contract.clone(),
            chain_id: sender_state.source_chain_id,
            block_height: Clock::get()?.slot,
            amount,
            sender: ctx.accounts.user.key().to_string(),
            receiver_address,
            nonce: new_nonce,
        });

        Ok(new_nonce)
    }

    pub fn add_relayer(ctx: Context<ManageRelayer>, relayer: Pubkey) -> Result<()> {
        let receiver_state = &mut ctx.accounts.receiver_state;

        require!(
            !receiver_state.relayers.contains(&relayer),
            ErrorCode::RelayerAlreadyExists
        );
        require!(
            receiver_state.relayers.len() < ReceiverState::MAX_RELAYERS,
            ErrorCode::TooManyRelayers
        );

        receiver_state.relayers.push(relayer);
        receiver_state.relayer_count += 1;

        Ok(())
    }

    pub fn remove_relayer(ctx: Context<ManageRelayer>, relayer: Pubkey) -> Result<()> {
        let receiver_state = &mut ctx.accounts.receiver_state;

        let index = receiver_state
            .relayers
            .iter()
            .position(|&r| r == relayer)
            .ok_or(ErrorCode::RelayerNotFound)?;

        receiver_state.relayers.remove(index);
        receiver_state.relayer_count -= 1;

        Ok(())
    }

    pub fn submit_signature(
        ctx: Context<SubmitSignature>,
        _nonce: u64,
        event_data: StakeEventData,
        signature: Vec<u8>,
    ) -> Result<()> {
        let receiver_state = &ctx.accounts.receiver_state;
        let cross_chain_request = &mut ctx.accounts.cross_chain_request;

        require!(
            receiver_state.usdc_mint != Pubkey::default(),
            ErrorCode::UsdcNotConfigured
        );

        require!(
            event_data.nonce > receiver_state.last_nonce,
            ErrorCode::InvalidNonce
        );

        let _relayer_index = receiver_state
            .relayers
            .iter()
            .position(|&r| r == ctx.accounts.relayer.key())
            .ok_or(ErrorCode::Unauthorized)?;

        if cross_chain_request.signature_count == 0 {
            cross_chain_request.nonce = event_data.nonce;
            cross_chain_request.signed_relayers = Vec::new();
            cross_chain_request.signature_count = 0;
            cross_chain_request.is_unlocked = false;
            cross_chain_request.event_data = event_data.clone();
        } else {
            require!(
                cross_chain_request.event_data == event_data,
                ErrorCode::InvalidEventData
            );
        }

        require!(
            !cross_chain_request.signed_relayers.contains(&ctx.accounts.relayer.key()),
            ErrorCode::RelayerAlreadySigned
        );

        let relayer_pubkey = ctx.accounts.relayer.key();
        verify_ed25519_signature(
            &ctx.accounts.instructions_sysvar,
            &event_data,
            &signature,
            &relayer_pubkey,
        )?;

        cross_chain_request.signed_relayers.push(ctx.accounts.relayer.key());
        cross_chain_request.signature_count += 1;

        let threshold = ((receiver_state.relayer_count * 2 + 2) / 3) as u8;

        if cross_chain_request.signature_count >= threshold && !cross_chain_request.is_unlocked {
            cross_chain_request.is_unlocked = true;

            let receiver_state = &mut ctx.accounts.receiver_state;
            receiver_state.last_nonce = cross_chain_request.event_data.nonce;

            let (vault_pda, vault_bump) = Pubkey::find_program_address(&[b"vault"], ctx.program_id);
            require!(vault_pda == ctx.accounts.vault.key(), ErrorCode::Unauthorized);

            let vault_seeds = &[b"vault".as_ref(), &[vault_bump]];
            let signer_seeds = &[&vault_seeds[..]];

            let cpi_accounts = TransferChecked {
                from: ctx.accounts.vault_token_account.to_account_info(),
                to: ctx.accounts.receiver_token_account.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
                mint: ctx.accounts.usdc_mint.to_account_info(),
            };
            let cpi_program = ctx.accounts.token_program.to_account_info();
            let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
            let unlock_amount = cross_chain_request.event_data.amount;
            token_interface::transfer_checked(cpi_ctx, unlock_amount, ctx.accounts.usdc_mint.decimals)?;

            let sender_bytes = &cross_chain_request.event_data.sender;
            let sender_hex = if sender_bytes[..12].iter().all(|&b| b == 0) {
                format!("0x{}", hex::encode(&sender_bytes[12..]))
            } else {
                format!("0x{}", hex::encode(sender_bytes))
            };

            emit!(CrossChainSuccessEvent {
                evm_address: sender_hex,
                amount: unlock_amount,
                nonce: cross_chain_request.event_data.nonce,
                source_chain_id: receiver_state.source_chain_id,
                block_height: cross_chain_request.event_data.block_height,
                receiver_address: cross_chain_request.event_data.receiver_address.to_string(),
            });
        }

        Ok(())
    }

    pub fn add_liquidity(ctx: Context<ManageLiquidity>, amount: u64) -> Result<()> {
        let cpi_accounts = TransferChecked {
            from: ctx.accounts.admin_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.admin.to_account_info(),
            mint: ctx.accounts.usdc_mint.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token_interface::transfer_checked(cpi_ctx, amount, ctx.accounts.usdc_mint.decimals)?;
        Ok(())
    }

    pub fn withdraw_liquidity(ctx: Context<ManageLiquidity>, amount: u64) -> Result<()> {
        let (vault_pda, vault_bump) = Pubkey::find_program_address(&[b"vault"], ctx.program_id);
        require!(vault_pda == ctx.accounts.vault.key(), ErrorCode::Unauthorized);

        let vault_seeds = &[b"vault".as_ref(), &[vault_bump]];
        let signer_seeds = &[&vault_seeds[..]];

        let cpi_accounts = TransferChecked {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.admin_token_account.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
            mint: ctx.accounts.usdc_mint.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
        token_interface::transfer_checked(cpi_ctx, amount, ctx.accounts.usdc_mint.decimals)?;
        Ok(())
    }
}

fn verify_ed25519_signature(
    instructions_sysvar: &AccountInfo,
    event_data: &StakeEventData,
    signature: &[u8],
    signer_pubkey: &Pubkey,
) -> Result<()> {
    use anchor_lang::solana_program::sysvar::instructions::{
        load_current_index_checked,
        load_instruction_at_checked,
    };

    let ed25519_program_id = Pubkey::new_from_array([
        3, 125, 70, 214, 124, 147, 251, 190,
        18, 249, 66, 143, 131, 141, 64, 255,
        5, 112, 116, 73, 39, 244, 138, 100,
        252, 202, 112, 68, 128, 0, 0, 0,
    ]);

    require!(signature.len() == 64, ErrorCode::InvalidSignature);

    let message = event_data
        .try_to_vec()
        .map_err(|_| ErrorCode::InvalidSignature)?;

    let current_index = load_current_index_checked(instructions_sysvar)
        .map_err(|_| ErrorCode::InvalidSignature)?;

    let mut found_ed25519_ix = false;

    for i in 0..current_index {
        let ix = load_instruction_at_checked(i as usize, instructions_sysvar)
            .map_err(|_| ErrorCode::InvalidSignature)?;

        if ix.program_id != ed25519_program_id {
            continue;
        }

        let data = &ix.data;
        require!(data.len() >= 16, ErrorCode::InvalidSignature);

        let num_signatures = data[0];
        require!(num_signatures == 1, ErrorCode::InvalidSignature);

        let sig_offset = u16::from_le_bytes([data[2], data[3]]) as usize;
        let pubkey_offset = u16::from_le_bytes([data[6], data[7]]) as usize;
        let msg_offset = u16::from_le_bytes([data[10], data[11]]) as usize;
        let msg_size = u16::from_le_bytes([data[12], data[13]]) as usize;

        require!(
            sig_offset + 64 <= data.len()
                && pubkey_offset + 32 <= data.len()
                && msg_offset + msg_size <= data.len(),
            ErrorCode::InvalidSignature
        );

        let ix_signature = &data[sig_offset..sig_offset + 64];
        let ix_pubkey = &data[pubkey_offset..pubkey_offset + 32];
        let ix_message = &data[msg_offset..msg_offset + msg_size];

        require!(ix_signature == signature, ErrorCode::InvalidSignature);
        require!(ix_pubkey == signer_pubkey.as_ref(), ErrorCode::InvalidSignature);
        require!(ix_message == message.as_slice(), ErrorCode::InvalidSignature);

        found_ed25519_ix = true;
        break;
    }

    require!(found_ed25519_ix, ErrorCode::InvalidSignature);

    Ok(())
}

// ============================================================
// Account Contexts
// ============================================================

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + SenderState::LEN,
        seeds = [b"sender_state"],
        bump
    )]
    pub sender_state: Account<'info, SenderState>,

    #[account(
        init,
        payer = admin,
        space = 8 + ReceiverState::LEN,
        seeds = [b"receiver_state"],
        bump
    )]
    pub receiver_state: Account<'info, ReceiverState>,

    #[account(mut)]
    pub admin: Signer<'info>,

    /// CHECK: Vault PDA, not a program account
    pub vault: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ConfigureUsdc<'info> {
    #[account(
        mut,
        seeds = [b"sender_state"],
        bump,
        has_one = admin @ ErrorCode::Unauthorized
    )]
    pub sender_state: Account<'info, SenderState>,

    #[account(
        mut,
        seeds = [b"receiver_state"],
        bump,
        has_one = admin @ ErrorCode::Unauthorized
    )]
    pub receiver_state: Account<'info, ReceiverState>,

    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct ConfigurePeer<'info> {
    #[account(
        mut,
        seeds = [b"sender_state"],
        bump,
        has_one = admin @ ErrorCode::Unauthorized
    )]
    pub sender_state: Account<'info, SenderState>,

    #[account(
        mut,
        seeds = [b"receiver_state"],
        bump,
        has_one = admin @ ErrorCode::Unauthorized
    )]
    pub receiver_state: Account<'info, ReceiverState>,

    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct ConfigureReceiverPeer<'info> {
    #[account(
        mut,
        seeds = [b"receiver_state"],
        bump,
        has_one = admin @ ErrorCode::Unauthorized
    )]
    pub receiver_state: Account<'info, ReceiverState>,

    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct Stake<'info> {
    #[account(
        mut,
        seeds = [b"sender_state"],
        bump
    )]
    pub sender_state: Account<'info, SenderState>,

    #[account(mut)]
    pub user: Signer<'info>,

    /// CHECK: Vault PDA
    pub vault: UncheckedAccount<'info>,

    pub usdc_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        constraint = user_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = user_token_account.owner == user.key() @ ErrorCode::Unauthorized
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        constraint = vault_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = vault_token_account.owner == vault.key() @ ErrorCode::Unauthorized
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ManageRelayer<'info> {
    #[account(
        mut,
        seeds = [b"receiver_state"],
        bump,
        has_one = admin @ ErrorCode::Unauthorized
    )]
    pub receiver_state: Account<'info, ReceiverState>,

    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct SubmitSignature<'info> {
    #[account(
        mut,
        seeds = [b"receiver_state"],
        bump
    )]
    pub receiver_state: Account<'info, ReceiverState>,

    #[account(
        init_if_needed,
        payer = relayer,
        space = 8 + CrossChainRequest::LEN,
        seeds = [b"cross_chain_request", nonce.to_le_bytes().as_ref()],
        bump
    )]
    pub cross_chain_request: Account<'info, CrossChainRequest>,

    #[account(mut)]
    pub relayer: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault"],
        bump
    )]
    /// CHECK: This is the vault PDA
    pub vault: UncheckedAccount<'info>,

    pub usdc_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        constraint = vault_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = vault_token_account.owner == vault.key() @ ErrorCode::Unauthorized
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    /// CHECK: This is the receiver token account
    pub receiver_token_account: UncheckedAccount<'info>,

    /// CHECK: This is the instructions sysvar account
    #[account(address = anchor_lang::solana_program::sysvar::instructions::ID)]
    pub instructions_sysvar: AccountInfo<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ManageLiquidity<'info> {
    #[account(
        seeds = [b"sender_state"],
        bump,
        has_one = admin @ ErrorCode::Unauthorized
    )]
    pub sender_state: Account<'info, SenderState>,

    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault"],
        bump
    )]
    /// CHECK: Vault PDA
    pub vault: UncheckedAccount<'info>,

    pub usdc_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        constraint = admin_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = admin_token_account.owner == admin.key() @ ErrorCode::Unauthorized
    )]
    pub admin_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        constraint = vault_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = vault_token_account.owner == vault.key() @ ErrorCode::Unauthorized
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

// ============================================================
// State Accounts
// ============================================================

#[account]
pub struct SenderState {
    pub vault: Pubkey,
    pub admin: Pubkey,
    pub usdc_mint: Pubkey,
    pub nonce: u64,
    pub target_contract: String,
    pub source_chain_id: u64,
    pub target_chain_id: u64,
}

impl SenderState {
    pub const LEN: usize = 32 + // vault
        32 + // admin
        32 + // usdc_mint
        8 + // nonce
        4 + 64 + // target_contract (String max 64 chars)
        8 + // source_chain_id
        8; // target_chain_id
}

#[account]
pub struct ReceiverState {
    pub vault: Pubkey,
    pub admin: Pubkey,
    pub usdc_mint: Pubkey,
    pub relayer_count: u64,
    pub source_contract: String,
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub relayers: Vec<Pubkey>,
    pub last_nonce: u64,
}

impl ReceiverState {
    pub const BASE_LEN: usize = 32 + // vault
        32 + // admin
        32 + // usdc_mint
        8 + // relayer_count
        4 + 64 + // source_contract (String max 64 chars)
        8 + // source_chain_id
        8 + // target_chain_id
        8; // last_nonce
    pub const MAX_RELAYERS: usize = 18;
    pub const LEN: usize = Self::BASE_LEN
        + 4 + (32 * Self::MAX_RELAYERS); // relayers Vec
}

#[account]
pub struct CrossChainRequest {
    pub nonce: u64,
    pub signed_relayers: Vec<Pubkey>,
    pub signature_count: u8,
    pub is_unlocked: bool,
    pub event_data: StakeEventData,
}

impl CrossChainRequest {
    pub const MAX_RELAYERS: usize = 18;
    pub const LEN: usize = 8 + // nonce
        4 + (32 * Self::MAX_RELAYERS) + // signed_relayers Vec
        1 + // signature_count
        1 + // is_unlocked
        StakeEventData::LEN; // event_data
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
    pub const LEN: usize =
        8 + // nonce
        8 + // amount
        8 + // block_height
        32 + // sender (unified 32-byte format)
        32; // receiver_address (Pubkey)
}

// ============================================================
// Events
// ============================================================

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
    pub evm_address: String,
    pub amount: u64,
    pub nonce: u64,
    pub source_chain_id: u64,
    pub block_height: u64,
    pub receiver_address: String,
}

// ============================================================
// Errors
// ============================================================

#[error_code]
pub enum ErrorCode {
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("USDC address not configured")]
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
    #[msg("Invalid event data: event data must match the first submitted event data")]
    InvalidEventData,
}
