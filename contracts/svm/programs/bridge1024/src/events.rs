use anchor_lang::prelude::*;

#[event]
pub struct StakeEvent {
    pub source_contract: [u8; 32],
    pub target_contract: [u8; 32],
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub block_height: u64,
    pub amount: u64,
    pub sender: [u8; 32],
    pub receiver: [u8; 32],
    pub nonce: u64,
}

#[event]
pub struct TokensUnlocked {
    pub nonce: u64,
    pub receiver: Pubkey,
    pub amount: u64,
    pub sender: [u8; 32],
}

#[event]
pub struct RelayerAdded {
    pub relayer: Pubkey,
}

#[event]
pub struct RelayerRemoved {
    pub relayer: Pubkey,
}

#[event]
pub struct EventConfirmed {
    pub relayer: Pubkey,
    pub nonce: u64,
}

#[event]
pub struct GuardianUpdated {
    pub old_guardian: Pubkey,
    pub new_guardian: Pubkey,
}

#[event]
pub struct AdminTransferProposed {
    pub current_admin: Pubkey,
    pub pending_admin: Pubkey,
}

#[event]
pub struct AdminTransferAccepted {
    pub old_admin: Pubkey,
    pub new_admin: Pubkey,
}

#[event]
pub struct BridgeConfigured {
    pub usdc_mint: Pubkey,
    pub peer_contract: [u8; 32],
    pub local_chain_id: u64,
    pub peer_chain_id: u64,
}

#[event]
pub struct RateLimitsConfigured {
    pub max_unlock_per_window: u64,
    pub window_duration: u64,
    pub max_single_unlock: u64,
    pub max_stake_amount: u64,
    pub minimum_reserve: u64,
}

#[event]
pub struct FeeConfigured {
    pub fee: u64,
}

#[event]
pub struct TokenWithdrawn {
    pub mint: Pubkey,
    pub to: Pubkey,
    pub amount: u64,
}

#[event]
pub struct OperatorUpdated {
    pub old_operator: Pubkey,
    pub new_operator: Pubkey,
}

#[event]
pub struct NonceSkipped {
    pub nonce: u64,
}

#[event]
pub struct Refunded {
    pub nonce: u64,
    pub to: Pubkey,
    pub amount: u64,
}

#[event]
pub struct EmergencyFreezeActivated {
    pub triggered_by: Pubkey,
}

#[event]
pub struct RecoveryExecuted {
    pub old_admin: Pubkey,
    pub new_admin: Pubkey,
}

#[event]
pub struct RecoveryUpdated {
    pub old_recovery: Pubkey,
    pub new_recovery: Pubkey,
}

#[event]
pub struct TimelockActivated {}

#[event]
pub struct OperationScheduled {
    pub op_hash: [u8; 32],
    pub eta: u64,
    pub data: Vec<u8>,
}

#[event]
pub struct OperationExecuted {
    pub op_hash: [u8; 32],
}

#[event]
pub struct OperationCancelled {
    pub op_hash: [u8; 32],
}
