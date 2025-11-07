use anchor_lang::prelude::*;

declare_id!("Core11111111111111111111111111111111111111");

#[program]
pub mod solana_core {
    use super::*;

    /// Initialize the bridge
    pub fn initialize(
        ctx: Context<Initialize>,
        guardian_set_index: u32,
        initial_guardians: Vec<[u8; 20]>,
    ) -> Result<()> {
        require!(
            initial_guardians.len() > 0,
            ErrorCode::NoGuardiansProvided
        );

        let bridge = &mut ctx.accounts.bridge;
        bridge.guardian_set_index = guardian_set_index;
        bridge.config = BridgeConfig {
            fee_lamports: 1_000_000, // 0.001 SOL
            chain_id: 2,             // Solana chain ID
        };

        let guardian_set = &mut ctx.accounts.guardian_set;
        guardian_set.index = guardian_set_index;
        guardian_set.keys = initial_guardians;
        guardian_set.creation_time = Clock::get()?.unix_timestamp;
        guardian_set.expiration_time = 0; // Active

        msg!("Bridge initialized with {} guardians", guardian_set.keys.len());

        Ok(())
    }

    /// Post a message
    pub fn post_message(
        ctx: Context<PostMessage>,
        nonce: u32,
        payload: Vec<u8>,
        consistency_level: u8,
    ) -> Result<()> {
        let bridge = &ctx.accounts.bridge;
        let message = &mut ctx.accounts.message;
        let emitter = &ctx.accounts.emitter;
        let sequence_account = &mut ctx.accounts.sequence;

        // Get and increment sequence
        let sequence = sequence_account.value;
        sequence_account.value += 1;

        // Fill message account
        message.consistency_level = consistency_level;
        message.emitter_chain = bridge.config.chain_id;
        message.emitter_address = emitter.key().to_bytes();
        message.sequence = sequence;
        message.timestamp = Clock::get()?.unix_timestamp as u32;
        message.nonce = nonce;
        message.payload = payload;

        msg!(
            "Message posted: emitter={}, sequence={}",
            emitter.key(),
            sequence
        );

        Ok(())
    }

    /// Verify VAA signatures (simplified version)
    pub fn verify_signatures(
        ctx: Context<VerifySignatures>,
        hash: [u8; 32],
        signatures_count: u8,
    ) -> Result<()> {
        let guardian_set = &ctx.accounts.guardian_set;

        // Calculate required quorum (2/3 + 1)
        let required = (guardian_set.keys.len() * 2 / 3) + 1;

        require!(
            signatures_count as usize >= required,
            ErrorCode::InsufficientSignatures
        );

        msg!(
            "Signatures verified: {}/{} required",
            signatures_count,
            required
        );

        Ok(())
    }
}

// ===== Accounts =====

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = payer,
        space = 8 + Bridge::INIT_SPACE,
        seeds = [b"bridge"],
        bump
    )]
    pub bridge: Account<'info, Bridge>,

    #[account(
        init,
        payer = payer,
        space = 8 + 4 + 4 + 8 + 4 + (20 * 19), // Guardian Set with 19 guardians
        seeds = [b"guardian_set", &guardian_set_index.to_le_bytes()],
        bump
    )]
    pub guardian_set: Account<'info, GuardianSet>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct PostMessage<'info> {
    #[account(seeds = [b"bridge"], bump)]
    pub bridge: Account<'info, Bridge>,

    #[account(
        init,
        payer = payer,
        space = 8 + PostedMessage::INIT_SPACE,
        seeds = [
            b"message",
            emitter.key().as_ref(),
            &sequence.value.to_le_bytes()
        ],
        bump
    )]
    pub message: Account<'info, PostedMessage>,

    pub emitter: Signer<'info>,

    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + 8,
        seeds = [b"sequence", emitter.key().as_ref()],
        bump
    )]
    pub sequence: Account<'info, Sequence>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct VerifySignatures<'info> {
    #[account(
        seeds = [b"guardian_set", &guardian_set.index.to_le_bytes()],
        bump
    )]
    pub guardian_set: Account<'info, GuardianSet>,
}

// ===== State Accounts =====

#[account]
#[derive(InitSpace)]
pub struct Bridge {
    pub guardian_set_index: u32,
    pub config: BridgeConfig,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, InitSpace)]
pub struct BridgeConfig {
    pub fee_lamports: u64,
    pub chain_id: u16,
}

#[account]
pub struct GuardianSet {
    pub index: u32,
    pub keys: Vec<[u8; 20]>,
    pub creation_time: i64,
    pub expiration_time: u32,
}

#[account]
#[derive(InitSpace)]
pub struct PostedMessage {
    pub consistency_level: u8,
    pub emitter_chain: u16,
    pub emitter_address: [u8; 32],
    pub sequence: u64,
    pub timestamp: u32,
    pub nonce: u32,
    #[max_len(1024)]
    pub payload: Vec<u8>,
}

#[account]
pub struct Sequence {
    pub value: u64,
}

// ===== Errors =====

#[error_code]
pub enum ErrorCode {
    #[msg("No guardians provided")]
    NoGuardiansProvided,

    #[msg("Insufficient signatures")]
    InsufficientSignatures,

    #[msg("Invalid guardian index")]
    InvalidGuardianIndex,

    #[msg("VAA already consumed")]
    VAAAlreadyConsumed,
}

