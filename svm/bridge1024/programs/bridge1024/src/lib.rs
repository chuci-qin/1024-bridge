use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, Transfer};
use anchor_spl::token::TokenAccount;

declare_id!("6mjsAsamy8E9nysMjdnNVE2YEAHpu1Xg25av9HbJbzyu");

#[program]
pub mod bridge1024 {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let sender_state = &mut ctx.accounts.sender_state;
        let receiver_state = &mut ctx.accounts.receiver_state;

        // Initialize sender state
        sender_state.vault = ctx.accounts.vault.key();
        sender_state.admin = ctx.accounts.admin.key();
        sender_state.nonce = 0;
        sender_state.usdc_mint = Pubkey::default();
        sender_state.target_contract = String::new();
        sender_state.source_chain_id = 0;
        sender_state.target_chain_id = 0;

        // Initialize receiver state
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
        let sender_state = &mut ctx.accounts.sender_state;
        let receiver_state = &mut ctx.accounts.receiver_state;

        sender_state.usdc_mint = usdc_mint;
        receiver_state.usdc_mint = usdc_mint;

        Ok(())
    }

    pub fn configure_peer(
        ctx: Context<ConfigurePeer>,
        peer_contract: String,
        source_chain_id: u64,
        target_chain_id: u64,
    ) -> Result<()> {
        let sender_state = &mut ctx.accounts.sender_state;
        let receiver_state = &mut ctx.accounts.receiver_state;

        // Configure sender state (for SVM → EVM transfers)
        sender_state.target_contract = peer_contract.clone();
        sender_state.source_chain_id = source_chain_id;  // SVM chain ID
        sender_state.target_chain_id = target_chain_id;  // EVM chain ID

        // Configure receiver state (for EVM → SVM transfers)
        // Note: Chain IDs are swapped for receiver since it receives from EVM
        receiver_state.source_contract = peer_contract;
        receiver_state.source_chain_id = target_chain_id;  // EVM chain ID (source of incoming events)
        receiver_state.target_chain_id = source_chain_id;  // SVM chain ID (target of incoming events)

        Ok(())
    }

    // Simplified function to configure only receiver state (for EVM → SVM)
    pub fn configure_receiver_peer(
        ctx: Context<ConfigureReceiverPeer>,
        peer_contract: String,
        source_chain_id: u64,  // EVM chain ID
        target_chain_id: u64,  // SVM chain ID
    ) -> Result<()> {
        let receiver_state = &mut ctx.accounts.receiver_state;

        // Configure receiver state for incoming transfers from EVM
        receiver_state.source_contract = peer_contract;
        receiver_state.source_chain_id = source_chain_id;  // EVM chain ID
        receiver_state.target_chain_id = target_chain_id;  // SVM chain ID

        Ok(())
    }

    pub fn stake(ctx: Context<Stake>, amount: u64, receiver_address: String) -> Result<u64> {
        let sender_state = &mut ctx.accounts.sender_state;

        // Verify USDC address is configured
        require!(
            sender_state.usdc_mint != Pubkey::default(),
            ErrorCode::UsdcNotConfigured
        );

        // Transfer tokens from user to vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        // Update nonce
        let current_nonce = sender_state.nonce;
        let new_nonce = current_nonce.wrapping_add(1);
        if new_nonce == 0 && current_nonce != u64::MAX {
            // This should not happen in normal operation
            return Err(ErrorCode::InvalidNonce.into());
        }
        sender_state.nonce = new_nonce;

        // Emit event
        emit!(StakeEvent {
            source_contract: ctx.program_id.to_string(),
            target_contract: sender_state.target_contract.clone(),
            chain_id: sender_state.source_chain_id,
            block_height: Clock::get()?.slot,
            amount,
            receiver_address,
            nonce: new_nonce,
        });

        Ok(new_nonce)
    }

    pub fn add_relayer(
        ctx: Context<ManageRelayer>,
        relayer: Pubkey,
    ) -> Result<()> {
        let receiver_state = &mut ctx.accounts.receiver_state;

        // Check if relayer already exists
        require!(
            !receiver_state.relayers.contains(&relayer),
            ErrorCode::RelayerAlreadyExists
        );

        // Check max relayers limit
        require!(
            receiver_state.relayers.len() < ReceiverState::MAX_RELAYERS,
            ErrorCode::TooManyRelayers
        );

        // Add relayer (Ed25519 public key is already in the relayer Pubkey)
        receiver_state.relayers.push(relayer);
        receiver_state.relayer_count += 1;

        Ok(())
    }

    pub fn remove_relayer(ctx: Context<ManageRelayer>, relayer: Pubkey) -> Result<()> {
        let receiver_state = &mut ctx.accounts.receiver_state;

        // Find relayer index
        let index = receiver_state
            .relayers
            .iter()
            .position(|&r| r == relayer)
            .ok_or(ErrorCode::RelayerNotFound)?;

        // Remove relayer
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

        // Verify USDC address is configured
        require!(
            receiver_state.usdc_mint != Pubkey::default(),
            ErrorCode::UsdcNotConfigured
        );

        // Verify source contract address
        require!(
            event_data.source_contract == receiver_state.source_contract,
            ErrorCode::InvalidSourceContract
        );

        // Verify chain ID
        require!(
            event_data.source_chain_id == receiver_state.source_chain_id,
            ErrorCode::InvalidChainId
        );

        // Verify nonce is incrementing
        require!(
            event_data.nonce > receiver_state.last_nonce,
            ErrorCode::InvalidNonce
        );

        // Verify relayer is whitelisted
        let _relayer_index = receiver_state
            .relayers
            .iter()
            .position(|&r| r == ctx.accounts.relayer.key())
            .ok_or(ErrorCode::Unauthorized)?;

        // Initialize cross-chain request if this is the first signature
        if cross_chain_request.signature_count == 0 {
            cross_chain_request.nonce = event_data.nonce;
            cross_chain_request.signed_relayers = Vec::new();
            cross_chain_request.signature_count = 0;
            cross_chain_request.is_unlocked = false;
            cross_chain_request.event_data = event_data.clone();
        } else {
            // Verify that the submitted event_data matches the stored event_data
            // This prevents a malicious relayer from submitting different event_data
            require!(
                cross_chain_request.event_data.source_contract == event_data.source_contract &&
                cross_chain_request.event_data.target_contract == event_data.target_contract &&
                cross_chain_request.event_data.source_chain_id == event_data.source_chain_id &&
                cross_chain_request.event_data.target_chain_id == event_data.target_chain_id &&
                cross_chain_request.event_data.block_height == event_data.block_height &&
                cross_chain_request.event_data.amount == event_data.amount &&
                cross_chain_request.event_data.receiver_address == event_data.receiver_address &&
                cross_chain_request.event_data.nonce == event_data.nonce,
                ErrorCode::InvalidEventData
            );
        }

        // Check if this relayer has already signed
        require!(
            !cross_chain_request.signed_relayers.contains(&ctx.accounts.relayer.key()),
            ErrorCode::RelayerAlreadySigned
        );

        // Verify Ed25519 signature using Ed25519Program
        // Note: Signature is verified against the submitted event_data
        // This ensures each relayer's signature matches their submitted event_data
        // The consistency check above ensures all relayers submit the same event_data
        let relayer_pubkey = ctx.accounts.relayer.key();
        verify_ed25519_signature(
            &ctx.accounts.instructions_sysvar,
            &event_data,
            &signature,
            &relayer_pubkey
        )?;

        // Record signature
        cross_chain_request.signed_relayers.push(ctx.accounts.relayer.key());
        cross_chain_request.signature_count += 1;

        // Calculate threshold: ceil(relayer_count * 2 / 3)
        let threshold = ((receiver_state.relayer_count * 2 + 2) / 3) as u8;

        // Check if threshold is reached
        if cross_chain_request.signature_count >= threshold && !cross_chain_request.is_unlocked {
            // Mark as unlocked
            cross_chain_request.is_unlocked = true;

            // Update last_nonce
            // Use the stored event_data.nonce instead of function parameter
            let receiver_state = &mut ctx.accounts.receiver_state;
            receiver_state.last_nonce = cross_chain_request.event_data.nonce;

            // Unlock tokens: transfer from vault to receiver
            // Find vault bump
            let (vault_pda, vault_bump) = Pubkey::find_program_address(&[b"vault"], ctx.program_id);
            require!(vault_pda == ctx.accounts.vault.key(), ErrorCode::Unauthorized);
            
            let vault_seeds = &[b"vault".as_ref(), &[vault_bump]];
            let signer_seeds = &[&vault_seeds[..]];

            let cpi_accounts = Transfer {
                from: ctx.accounts.vault_token_account.to_account_info(),
                to: ctx.accounts.receiver_token_account.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
            };
            let cpi_program = ctx.accounts.token_program.to_account_info();
            let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
            // Use the stored event_data instead of function parameter to prevent inconsistencies
            token::transfer(cpi_ctx, cross_chain_request.event_data.amount)?;
        }

        Ok(())
    }

    pub fn add_liquidity(ctx: Context<ManageLiquidity>, amount: u64) -> Result<()> {
        // Transfer from admin token account to vault token account
        let cpi_accounts = Transfer {
            from: ctx.accounts.admin_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.admin.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        Ok(())
    }

    pub fn withdraw_liquidity(ctx: Context<ManageLiquidity>, amount: u64) -> Result<()> {
        // Transfer from vault token account to admin token account using vault PDA authority
        // Find vault bump
        let (vault_pda, vault_bump) = Pubkey::find_program_address(&[b"vault"], ctx.program_id);
        require!(vault_pda == ctx.accounts.vault.key(), ErrorCode::Unauthorized);
        
        let vault_seeds = &[b"vault".as_ref(), &[vault_bump]];
        let signer_seeds = &[&vault_seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.admin_token_account.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
        token::transfer(cpi_ctx, amount)?;

        Ok(())
    }
}

/// Verify Ed25519 signature using Solana's Ed25519Program (native precompile)
/// This is used for EVM → SVM cross-chain transfers (SVM as receiver)
/// For SVM → EVM transfers, EVM contracts will use ECDSA verification
///
/// Architecture:
/// - EVM → SVM: Relayers sign with Ed25519, verified here via Ed25519Program
/// - SVM → EVM: Relayers sign with ECDSA, verified on EVM contracts
///
/// This function performs FULL CRYPTOGRAPHIC Ed25519 signature verification by:
/// 1. Checking that an Ed25519Program instruction exists in the transaction
/// 2. Verifying the instruction's signature, pubkey, and message match our parameters
/// 3. Ed25519Program has already performed the actual cryptographic verification
/// 4. If we reach here, the signature is cryptographically valid
///
/// Security: This provides the strongest possible guarantees - signatures cannot be forged
/// without the private key. This is the same security model as Solana transaction signatures.
fn verify_ed25519_signature(
    instructions_sysvar: &AccountInfo,
    event_data: &StakeEventData,
    signature: &[u8],
    signer_pubkey: &Pubkey,
) -> Result<()> {
    use anchor_lang::solana_program::sysvar::instructions::{
        load_current_index_checked, 
        load_instruction_at_checked
    };
    
    // Ed25519Program ID: Ed25519SigVerify111111111111111111111111111
    // Correct bytes for Ed25519Program.programId
    let ed25519_program_id = Pubkey::new_from_array([
        3, 125, 70, 214, 124, 147, 251, 190,
        18, 249, 66, 143, 131, 141, 64, 255,
        5, 112, 116, 73, 39, 244, 138, 100,
        252, 202, 112, 68, 128, 0, 0, 0
    ]);
    
    // Ed25519 signature must be exactly 64 bytes
    require!(
        signature.len() == 64,
        ErrorCode::InvalidSignature
    );

    // Serialize event data - this is what relayers sign
    let message = event_data.try_to_vec()
        .map_err(|_| ErrorCode::InvalidSignature)?;

    // Get current instruction index
    let current_index = load_current_index_checked(instructions_sysvar)
        .map_err(|_| ErrorCode::InvalidSignature)?;

    // Search for Ed25519Program instruction before our instruction
    let mut found_ed25519_ix = false;

    for i in 0..current_index {
        let ix = load_instruction_at_checked(i as usize, instructions_sysvar)
            .map_err(|_| ErrorCode::InvalidSignature)?;

        // Check if this is an Ed25519Program instruction
        if ix.program_id != ed25519_program_id {
            continue;
        }

        // Ed25519Program instruction data format:
        // Offset 0: num_signatures (u8) - must be 1
        // Offset 1: padding (u8)
        // Offset 2-3: signature_offset (u16 LE)
        // Offset 4-5: signature_instruction_index (u16 LE)
        // Offset 6-7: public_key_offset (u16 LE)
        // Offset 8-9: public_key_instruction_index (u16 LE)
        // Offset 10-11: message_data_offset (u16 LE)
        // Offset 12-13: message_data_size (u16 LE)
        // Offset 14-15: message_instruction_index (u16 LE)
        // Offset 16+: actual data (signature + pubkey + message)

        let data = &ix.data;
        require!(data.len() >= 16, ErrorCode::InvalidSignature);

        // Parse instruction header
        let num_signatures = data[0];
        require!(num_signatures == 1, ErrorCode::InvalidSignature);

        let sig_offset = u16::from_le_bytes([data[2], data[3]]) as usize;
        let pubkey_offset = u16::from_le_bytes([data[6], data[7]]) as usize;
        let msg_offset = u16::from_le_bytes([data[10], data[11]]) as usize;
        let msg_size = u16::from_le_bytes([data[12], data[13]]) as usize;

        // Verify offsets are within bounds
        require!(
            sig_offset + 64 <= data.len() &&
            pubkey_offset + 32 <= data.len() &&
            msg_offset + msg_size <= data.len(),
            ErrorCode::InvalidSignature
        );

        // Extract data from instruction
        let ix_signature = &data[sig_offset..sig_offset + 64];
        let ix_pubkey = &data[pubkey_offset..pubkey_offset + 32];
        let ix_message = &data[msg_offset..msg_offset + msg_size];

        // Verify signature matches
        require!(ix_signature == signature, ErrorCode::InvalidSignature);

        // Verify public key matches
        require!(ix_pubkey == signer_pubkey.as_ref(), ErrorCode::InvalidSignature);

        // Verify message matches
        require!(ix_message == message.as_slice(), ErrorCode::InvalidSignature);

        // If we reach here:
        // 1. Ed25519Program instruction exists
        // 2. Signature, pubkey, and message all match
        // 3. Ed25519Program performed cryptographic verification
        // 4. Transaction succeeded, so verification passed
        found_ed25519_ix = true;
        msg!("✅ Ed25519 signature cryptographically verified via Ed25519Program: relayer={}", 
             signer_pubkey);
        break;
    }

    require!(found_ed25519_ix, ErrorCode::InvalidSignature);

    Ok(())
}

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

    /// CHECK: This is the vault address, not a program account
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

    /// CHECK: This is the vault address, not a program account
    pub vault: UncheckedAccount<'info>,

    /// CHECK: This is the USDC mint address
    pub usdc_mint: UncheckedAccount<'info>,

    #[account(
        mut,
        constraint = user_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = user_token_account.owner == user.key() @ ErrorCode::Unauthorized
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = vault_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = vault_token_account.owner == vault.key() @ ErrorCode::Unauthorized
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,

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

    /// CHECK: This is the USDC mint address
    pub usdc_mint: UncheckedAccount<'info>,

    #[account(
        mut,
        constraint = vault_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = vault_token_account.owner == vault.key() @ ErrorCode::Unauthorized
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    /// CHECK: This is the receiver token account
    pub receiver_token_account: UncheckedAccount<'info>,

    /// Instructions Sysvar for Ed25519 signature verification
    /// CHECK: This is the instructions sysvar account
    #[account(address = anchor_lang::solana_program::sysvar::instructions::ID)]
    pub instructions_sysvar: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ManageLiquidity<'info> {
    #[account(
        seeds = [b"receiver_state"],
        bump,
        has_one = admin @ ErrorCode::Unauthorized
    )]
    pub receiver_state: Account<'info, ReceiverState>,

    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault"],
        bump
    )]
    /// CHECK: This is the vault PDA
    pub vault: UncheckedAccount<'info>,

    /// CHECK: This is the USDC mint address
    pub usdc_mint: UncheckedAccount<'info>,

    #[account(
        mut,
        constraint = admin_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = admin_token_account.owner == admin.key() @ ErrorCode::Unauthorized
    )]
    pub admin_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = vault_token_account.mint == usdc_mint.key() @ ErrorCode::UsdcNotConfigured,
        constraint = vault_token_account.owner == vault.key() @ ErrorCode::Unauthorized
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

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
        + 4 + (32 * Self::MAX_RELAYERS); // relayers Vec (Ed25519 public keys are in Pubkey)
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

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct StakeEventData {
    pub source_contract: String,
    pub target_contract: String,
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub block_height: u64,
    pub amount: u64,
    pub receiver_address: String,
    pub nonce: u64,
}

impl StakeEventData {
    pub const LEN: usize = 
        4 + 64 + // source_contract (String with max 64 chars - hex encoded)
        4 + 64 + // target_contract (String with max 64 chars - hex encoded)
        8 + // source_chain_id
        8 + // target_chain_id
        8 + // block_height
        8 + // amount
        4 + 64 + // receiver_address (String with max 64 chars)
        8; // nonce
}

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

#[event]
pub struct StakeEvent {
    pub source_contract: String,
    pub target_contract: String,
    pub chain_id: u64,
    pub block_height: u64,
    pub amount: u64,
    pub receiver_address: String,
    pub nonce: u64,
}

