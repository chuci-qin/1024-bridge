use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, MintTo, Burn};

// 1024Chain Testnet的Token Program ID（不是标准ID）
// 注意：在使用时需要手动创建PublicKey
// Token Program: BTNcjvKBL5A2p5an7hB2SY8whcJSKxDv5uv5viM8hYvg
// ATA Program: 61hg92qNdABF1PUupwfLRnvmHd9zVAv4QaGZzYx2U9ER

// 程序ID（部署后会生成真实ID）
declare_id!("Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS");

#[program]
pub mod bridge {
    use super::*;

    /// 初始化Bridge Program
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let bridge_state = &mut ctx.accounts.bridge_state;
        bridge_state.authority = ctx.accounts.authority.key();
        bridge_state.nonce = 0;
        bridge_state.total_minted = 0;
        bridge_state.total_burned = 0;
        
        msg!("Bridge initialized by {}", ctx.accounts.authority.key());
        Ok(())
    }

    /// Mint USDC（充值到账）
    /// Relayer监听到Arbitrum Lock事件后调用此指令
    pub fn mint_usdc(
        ctx: Context<MintUSDC>,
        amount: u64,
        arb_tx_hash: String,  // Arbitrum交易哈希（用于防重放）
    ) -> Result<()> {
        require!(amount > 0, BridgeError::InvalidAmount);
        require!(!arb_tx_hash.is_empty(), BridgeError::InvalidTxHash);
        
        let bridge_state = &mut ctx.accounts.bridge_state;
        
        // 1. 检查是否已处理（防止重放攻击）
        require!(
            !bridge_state.is_processed(&arb_tx_hash),
            BridgeError::AlreadyProcessed
        );
        
        // 2. Mint USDC到用户账户
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.usdc_mint.to_account_info(),
                to: ctx.accounts.user_usdc_account.to_account_info(),
                authority: ctx.accounts.bridge_authority.to_account_info(),
            },
        );
        
        token::mint_to(cpi_ctx, amount)?;
        
        // 3. 记录已处理
        bridge_state.processed_deposits.push(arb_tx_hash.clone());
        bridge_state.total_minted = bridge_state.total_minted.checked_add(amount)
            .ok_or(BridgeError::Overflow)?;
        
        msg!("Minted {} USDC to {}, arb_tx: {}", 
            amount, 
            ctx.accounts.user_usdc_account.key(), 
            arb_tx_hash
        );
        
        Ok(())
    }

    /// Burn USDC（提现申请）
    /// 用户申请提现时调用，Relayer监听此事件
    pub fn burn_usdc(
        ctx: Context<BurnUSDC>,
        amount: u64,
        arb_address: String,  // 目标Arbitrum地址（0x...）
    ) -> Result<()> {
        require!(amount > 0, BridgeError::InvalidAmount);
        require!(arb_address.starts_with("0x"), BridgeError::InvalidAddress);
        require!(arb_address.len() == 42, BridgeError::InvalidAddress);
        
        let bridge_state = &mut ctx.accounts.bridge_state;
        
        // 1. Burn用户的USDC
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Burn {
                mint: ctx.accounts.usdc_mint.to_account_info(),
                from: ctx.accounts.user_usdc_account.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        
        token::burn(cpi_ctx, amount)?;
        
        // 2. 更新统计
        bridge_state.total_burned = bridge_state.total_burned.checked_add(amount)
            .ok_or(BridgeError::Overflow)?;
        
        // 3. 发出日志（Relayer通过解析logs获取提现请求）
        msg!("WITHDRAW:{}:{}:{}", 
            ctx.accounts.user.key(), 
            arb_address, 
            amount
        );
        
        Ok(())
    }
}

// ========================================
// 账户结构
// ========================================

/// 初始化上下文
#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + BridgeState::SPACE,
        seeds = [b"bridge-state"],
        bump
    )]
    pub bridge_state: Account<'info, BridgeState>,
    
    #[account(mut)]
    pub authority: Signer<'info>,
    
    pub system_program: Program<'info, System>,
}

/// Mint USDC上下文
#[derive(Accounts)]
pub struct MintUSDC<'info> {
    #[account(
        mut,
        seeds = [b"bridge-state"],
        bump
    )]
    pub bridge_state: Account<'info, BridgeState>,
    
    #[account(mut)]
    pub usdc_mint: Account<'info, Mint>,
    
    #[account(mut)]
    pub user_usdc_account: Account<'info, TokenAccount>,
    
    /// CHECK: Bridge authority（PDA，有mint权限）
    #[account(
        seeds = [b"bridge-authority"],
        bump
    )]
    pub bridge_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

/// Burn USDC上下文
#[derive(Accounts)]
pub struct BurnUSDC<'info> {
    #[account(
        mut,
        seeds = [b"bridge-state"],
        bump
    )]
    pub bridge_state: Account<'info, BridgeState>,
    
    #[account(mut)]
    pub usdc_mint: Account<'info, Mint>,
    
    #[account(mut)]
    pub user_usdc_account: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub user: Signer<'info>,
    
    pub token_program: Program<'info, Token>,
}

// ========================================
// 状态账户
// ========================================

#[account]
pub struct BridgeState {
    pub authority: Pubkey,                   // 管理员
    pub nonce: u64,                          // 计数器
    pub processed_deposits: Vec<String>,     // 已处理的Arbitrum tx hash
    pub total_minted: u64,                   // 总铸造量
    pub total_burned: u64,                   // 总销毁量
}

impl BridgeState {
    pub const SPACE: usize = 32              // authority
        + 8                                   // nonce
        + 4 + (64 * 100)                     // processed_deposits（最多100个）
        + 8                                   // total_minted
        + 8;                                  // total_burned
    
    pub fn is_processed(&self, tx_hash: &str) -> bool {
        self.processed_deposits.contains(&tx_hash.to_string())
    }
}

// ========================================
// 错误定义
// ========================================

#[error_code]
pub enum BridgeError {
    #[msg("Transaction already processed")]
    AlreadyProcessed,
    
    #[msg("Invalid amount (must be > 0)")]
    InvalidAmount,
    
    #[msg("Invalid Arbitrum address")]
    InvalidAddress,
    
    #[msg("Invalid transaction hash")]
    InvalidTxHash,
    
    #[msg("Arithmetic overflow")]
    Overflow,
}

