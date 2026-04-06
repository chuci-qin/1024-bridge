use anchor_lang::prelude::*;

#[error_code]
pub enum ErrorCode {
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("Zero address")]
    ZeroAddress,
    #[msg("USDC mint not configured")]
    UsdcNotConfigured,
    #[msg("Relayer already exists")]
    RelayerAlreadyExists,
    #[msg("Too many relayers")]
    TooManyRelayers,
    #[msg("Relayer not found")]
    RelayerNotFound,
    #[msg("Already processed")]
    AlreadyProcessed,
    #[msg("Invalid source contract")]
    InvalidSourceContract,
    #[msg("Invalid target contract")]
    InvalidTargetContract,
    #[msg("Invalid source chain ID")]
    InvalidSourceChainId,
    #[msg("Invalid target chain ID")]
    InvalidTargetChainId,
    #[msg("Rate limit exceeded")]
    RateLimitExceeded,
    #[msg("Single transfer limit exceeded")]
    SingleTransferExceeded,
    #[msg("Insufficient reserve")]
    InsufficientReserve,
    #[msg("Relayer already confirmed")]
    RelayerAlreadyConfirmed,
    #[msg("Zero amount")]
    ZeroAmount,
    #[msg("Stake amount exceeded")]
    StakeAmountExceeded,
    #[msg("Already refunded")]
    AlreadyRefunded,
    #[msg("Timelock not scheduled")]
    TimelockNotScheduled,
    #[msg("Timelock not ready")]
    TimelockNotReady,
    #[msg("Timelock already scheduled")]
    TimelockAlreadyScheduled,
    #[msg("Timelock already active")]
    TimelockAlreadyActive,
    #[msg("Timelock not active")]
    TimelockNotActive,
    #[msg("Timelock expired")]
    TimelockExpired,
    #[msg("Invalid rate limit params")]
    InvalidRateLimitParams,
    #[msg("Invalid chain ID")]
    InvalidChainId,
    #[msg("Invalid receiver")]
    InvalidReceiver,
    #[msg("Role overlap")]
    RoleOverlap,
    #[msg("Bridge is paused")]
    Paused,
    #[msg("Bridge is not paused")]
    NotPaused,
    #[msg("Nonce mismatch")]
    NonceMismatch,
    #[msg("Receiver token account mint mismatch")]
    ReceiverMintMismatch,
    #[msg("Fee too high")]
    FeeTooHigh,
    #[msg("Insufficient balance")]
    InsufficientBalance,
    #[msg("Invalid event data")]
    InvalidEventData,
}
