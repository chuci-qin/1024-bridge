use std::env;

use crate::types::ChainKind;

/// Static metadata for a supported chain.
#[derive(Clone, Debug)]
pub struct ChainInfo {
    pub chain_id: u64,
    /// Used as the `RPC_<ENV_NAME>` override key.
    pub env_name: &'static str,
    pub default_rpc: &'static str,
    pub kind: ChainKind,
}

const CHAINS: &[ChainInfo] = &[
    // Ethereum
    ChainInfo {
        chain_id: 1,
        env_name: "ETHEREUM_MAINNET",
        default_rpc: "https://ethereum-rpc.publicnode.com",
        kind: ChainKind::Evm,
    },
    ChainInfo {
        chain_id: 11155111,
        env_name: "ETHEREUM_SEPOLIA",
        default_rpc: "https://ethereum-sepolia-rpc.publicnode.com",
        kind: ChainKind::Evm,
    },
    // Arbitrum
    ChainInfo {
        chain_id: 42161,
        env_name: "ARBITRUM_MAINNET",
        default_rpc: "https://arbitrum-one-rpc.publicnode.com",
        kind: ChainKind::Evm,
    },
    ChainInfo {
        chain_id: 421614,
        env_name: "ARBITRUM_SEPOLIA",
        default_rpc: "https://sepolia-rollup.arbitrum.io/rpc",
        kind: ChainKind::Evm,
    },
    // Base
    ChainInfo {
        chain_id: 8453,
        env_name: "BASE_MAINNET",
        default_rpc: "https://mainnet.base.org",
        kind: ChainKind::Evm,
    },
    ChainInfo {
        chain_id: 84532,
        env_name: "BASE_SEPOLIA",
        default_rpc: "https://sepolia.base.org",
        kind: ChainKind::Evm,
    },
    // Solana
    ChainInfo {
        chain_id: 101,
        env_name: "SOLANA_MAINNET",
        default_rpc: "https://api.mainnet-beta.solana.com",
        kind: ChainKind::Svm,
    },
    ChainInfo {
        chain_id: 103,
        env_name: "SOLANA_DEVNET",
        default_rpc: "https://api.devnet.solana.com",
        kind: ChainKind::Svm,
    },
    // 1024 Chain
    ChainInfo {
        chain_id: 91024,
        env_name: "1024_MAINNET",
        default_rpc: "https://rpc.1024chain.com",
        kind: ChainKind::Svm,
    },
    ChainInfo {
        chain_id: 91025,
        env_name: "1024_TESTNET",
        default_rpc: "https://rpc-testnet.1024chain.com/rpc/",
        kind: ChainKind::Svm,
    },
    ChainInfo {
        chain_id: 91026,
        env_name: "1024_STABLENET",
        default_rpc: "https://rpc-testnet-stable.1024chain.com",
        kind: ChainKind::Svm,
    },
];

/// Look up chain info by chain_id.
pub fn get_chain_info(chain_id: u64) -> Option<&'static ChainInfo> {
    CHAINS.iter().find(|c| c.chain_id == chain_id)
}

/// Resolve the effective RPC URL for a chain, checking `RPC_<ENV_NAME>` first.
pub fn resolve_rpc(info: &ChainInfo) -> String {
    let env_key = format!("RPC_{}", info.env_name);
    env::var(&env_key).unwrap_or_else(|_| info.default_rpc.to_string())
}

/// Map the `BRIDGE_1024_NETWORK` value to the corresponding 1024 chain_id.
pub fn network_to_chain_id(network: &str) -> Option<u64> {
    match network.to_lowercase().as_str() {
        "mainnet" => Some(91024),
        "testnet" => Some(91025),
        "stablenet" | "stable" => Some(91026),
        _ => None,
    }
}
