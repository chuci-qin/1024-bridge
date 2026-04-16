//! 链注册表模块
//!
//! 维护一个硬编码的链信息表，包含所有支持的 EVM 和 SVM 链：
//! - chain_id：链的唯一标识（EVM 用官方 chain_id，SVM 用自定义约定）
//! - env_name：RPC 环境变量的后缀名（如 RPC_ETHEREUM_MAINNET）
//! - default_rpc：免费/公共 RPC 的默认 URL
//! - kind：链类型（EVM 或 SVM）
//!
//! RPC 选择优先级：环境变量 `RPC_{env_name}` > 默认 RPC URL

use std::env;

use crate::types::ChainKind;

/// 一条链的静态元数据
#[derive(Clone, Debug)]
pub struct ChainInfo {
    /// 链唯一标识
    pub chain_id: u64,
    /// 用于环境变量覆盖 RPC 的键名后缀（如 "ETHEREUM_MAINNET" → 环境变量 RPC_ETHEREUM_MAINNET）
    pub env_name: &'static str,
    /// 公共 RPC 的默认 URL
    pub default_rpc: &'static str,
    /// 虚拟机类型
    pub kind: ChainKind,
}

/// 所有支持的链列表（硬编码）
const CHAINS: &[ChainInfo] = &[
    // ── Ethereum ──
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
    // ── Arbitrum ──
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
    // ── Base ──
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
    // ── Solana ──
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
    // ── 1024 Chain ──
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

/// 根据 chain_id 查找链信息。找不到返回 None。
pub fn get_chain_info(chain_id: u64) -> Option<&'static ChainInfo> {
    CHAINS.iter().find(|c| c.chain_id == chain_id)
}

/// 解析最终使用的 RPC URL：优先检查环境变量 `RPC_{env_name}`，否则用默认值。
///
/// 例如对 Ethereum Mainnet（env_name = "ETHEREUM_MAINNET"），
/// 会先检查 `RPC_ETHEREUM_MAINNET` 环境变量。
pub fn resolve_rpc(info: &ChainInfo) -> String {
    let env_key = format!("RPC_{}", info.env_name);
    env::var(&env_key).unwrap_or_else(|_| info.default_rpc.to_string())
}

/// 将 `BRIDGE_1024_NETWORK` 的值映射为 1024 链的 chain_id。
///
/// - "mainnet"  → 91024
/// - "testnet"  → 91025
/// - "stablenet" / "stable" → 91026
pub fn network_to_chain_id(network: &str) -> Option<u64> {
    match network.to_lowercase().as_str() {
        "mainnet" => Some(91024),
        "testnet" => Some(91025),
        "stablenet" | "stable" => Some(91026),
        _ => None,
    }
}
