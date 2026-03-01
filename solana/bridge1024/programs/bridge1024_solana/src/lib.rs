use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("CKRCgMnF7wgsrYFc4FT2WZYiWy3NQpCd3KGjvQWHruMS");

#[program]
pub mod bridge1024_solana {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let sender_state = &mut ctx.accounts.sender_state;

        sender_state.vault = ctx.accounts.vault.key();
        sender_state.admin = ctx.accounts.admin.key();
        sender_state.nonce = 0;
        sender_state.usdc_mint = Pubkey::default();
        sender_state.target_contract = String::new();
        sender_state.source_chain_id = 0;
        sender_state.target_chain_id = 0;

        Ok(())
    }

    pub fn configure_usdc(ctx: Context<ConfigureUsdc>, usdc_mint: Pubkey) -> Result<()> {
        ctx.accounts.sender_state.usdc_mint = usdc_mint;
        Ok(())
    }

    pub fn configure_peer(
        ctx: Context<ConfigurePeer>,
        target_contract: String,
        source_chain_id: u64,
        target_chain_id: u64,
    ) -> Result<()> {
        let sender_state = &mut ctx.accounts.sender_state;
        sender_state.target_contract = target_contract;
        sender_state.source_chain_id = source_chain_id;
        sender_state.target_chain_id = target_chain_id;
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

// ============================================================
// Errors
// ============================================================

#[error_code]
pub enum ErrorCode {
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("USDC address not configured")]
    UsdcNotConfigured,
    #[msg("Invalid nonce")]
    InvalidNonce,
}
