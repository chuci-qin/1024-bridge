use thiserror::Error;

pub type Result<T> = std::result::Result<T, RelayerError>;

#[derive(Error, Debug)]
pub enum RelayerError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("RPC connection error: {0}")]
    RpcConnection(String),

    #[error("RPC timeout")]
    RpcTimeout,

    #[error("Network error: {0}")]
    Network(String),

    #[error("Signature error: {0}")]
    Signature(String),

    #[error("Invalid event data: {0}")]
    InvalidEvent(String),

    #[error("Invalid nonce: expected > {expected}, got {actual}")]
    InvalidNonce { expected: u64, actual: u64 },

    #[error("Nonce already processed: {0}")]
    NonceAlreadyProcessed(u64),

    #[error("Invalid contract address")]
    InvalidContract,

    #[error("Invalid chain ID: expected {expected}, got {actual}")]
    InvalidChainId { expected: u64, actual: u64 },

    #[error("Relayer not whitelisted")]
    NotWhitelisted,

    #[error("Insufficient funds for gas: {0}")]
    InsufficientFunds(String),

    #[error("USDC not configured")]
    UsdcNotConfigured,

    #[error("Gas estimation failed: {0}")]
    GasEstimation(String),

    #[error("Transaction failed: {0}")]
    TransactionFailed(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Queue error: {0}")]
    Queue(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl RelayerError {
    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            RelayerError::RpcTimeout
                | RelayerError::RpcConnection(_)
                | RelayerError::Network(_)
                | RelayerError::GasEstimation(_)
        )
    }
}

