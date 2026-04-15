use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum RelayerError {
    #[error("Chain {0} not found in registry and no RPC override set")]
    UnknownChain(u64),

    #[error("Our relayer key is not in the bridge's relayer whitelist")]
    NotWhitelisted,

    #[error("No peers discovered from on-chain config")]
    NoPeers,

    #[error("Failed to deserialize on-chain account: {0}")]
    Deserialize(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("ABI encoding error: {0}")]
    Abi(String),
}
