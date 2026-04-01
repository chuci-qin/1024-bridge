use std::collections::HashMap;
use std::fmt;
use std::fs;

use serde::{Deserialize, Serialize};

use crate::error::BridgeError;
use crate::types::BridgeDirection;

/// Top-level bridges configuration (parses a flat JSON object keyed by bridge id).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgesConfig {
    #[serde(flatten)]
    pub bridges: HashMap<String, BridgeEntry>,
}

/// A single bridge definition (e.g. "USDT", "SOL-SOL").
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeEntry {
    pub token: String,
    pub decimal_ratio: u64,
    pub liquidity_amount: String,
    #[serde(rename = "type")]
    pub bridge_type: Option<String>,
    pub source: Option<ChainConfig>,
    pub target: Option<ChainConfig>,
    pub evm: Option<ChainConfig>,
    pub svm: Option<ChainConfig>,
    pub solana: Option<ChainConfig>,
}

/// Configuration for one side of a bridge (EVM chain or SVM chain).
#[derive(Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub rpc_url: String,
    pub token_address: String,
    pub token_decimals: u8,
    pub confirmation_blocks: Option<u32>,
    pub commitment: Option<String>,
    pub explorer_url: Option<String>,
    pub native_token_symbol: Option<String>,
}

/// REL-H2: redact rpc_url in Debug output to avoid leaking API keys.
impl fmt::Debug for ChainConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChainConfig")
            .field("name", &self.name)
            .field("chain_id", &self.chain_id)
            .field("rpc_url", &"<redacted>")
            .field("token_address", &self.token_address)
            .field("token_decimals", &self.token_decimals)
            .field("confirmation_blocks", &self.confirmation_blocks)
            .field("commitment", &self.commitment)
            .field("explorer_url", &self.explorer_url)
            .field("native_token_symbol", &self.native_token_symbol)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// BridgesConfig
// ---------------------------------------------------------------------------

impl BridgesConfig {
    /// Load and parse a `bridges.json` file.
    pub fn load(path: &str) -> Result<Self, BridgeError> {
        let contents = fs::read_to_string(path)
            .map_err(|e| BridgeError::Config(format!("Failed to read {path}: {e}")))?;
        Self::from_json(&contents)
    }

    /// Parse from a JSON string (useful for tests and embedded configs).
    pub fn from_json(json: &str) -> Result<Self, BridgeError> {
        serde_json::from_str(json)
            .map_err(|e| BridgeError::Config(format!("Failed to parse bridges config: {e}")))
    }

    pub fn get_bridge(&self, id: &str) -> Option<&BridgeEntry> {
        self.bridges.get(id)
    }
}

// ---------------------------------------------------------------------------
// BridgeEntry
// ---------------------------------------------------------------------------

impl BridgeEntry {
    /// Determine bridge direction from the explicit `type` field or legacy field presence.
    pub fn direction(&self) -> BridgeDirection {
        if let Some(ref bt) = self.bridge_type {
            match bt.to_lowercase().replace('-', "_").as_str() {
                "evm_to_svm" | "evmtosvm" => return BridgeDirection::EvmToSvm,
                "svm_to_evm" | "svmtoevm" => return BridgeDirection::SvmToEvm,
                "svm_to_svm" | "svmtosvm" => return BridgeDirection::SvmToSvm,
                _ => {}
            }
        }
        if self.evm.is_some() && (self.svm.is_some() || self.solana.is_some()) {
            BridgeDirection::EvmToSvm
        } else if self.solana.is_some() && self.evm.is_none() {
            BridgeDirection::SvmToSvm
        } else {
            BridgeDirection::EvmToSvm
        }
    }

    /// Return the source-side chain config (prefers `source`, falls back to `evm` then `solana`).
    pub fn source_config(&self) -> Option<&ChainConfig> {
        self.source
            .as_ref()
            .or(self.evm.as_ref())
            .or(self.solana.as_ref())
    }

    /// Return the target-side chain config (prefers `target`, falls back to `svm` then `solana`).
    pub fn target_config(&self) -> Option<&ChainConfig> {
        self.target
            .as_ref()
            .or(self.svm.as_ref())
            .or(self.solana.as_ref())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{
        "USDT": {
            "token": "USDT",
            "decimal_ratio": 1000000000000,
            "liquidity_amount": "1000000",
            "type": "evm_to_svm",
            "evm": {
                "name": "Ethereum",
                "chain_id": 1,
                "rpc_url": "https://mainnet.infura.io/v3/secret",
                "token_address": "0xdac17f958d2ee523a2206206994597c13d831ec7",
                "token_decimals": 6,
                "confirmation_blocks": 12
            },
            "svm": {
                "name": "Solana",
                "chain_id": 0,
                "rpc_url": "https://api.mainnet-beta.solana.com",
                "token_address": "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
                "token_decimals": 6,
                "commitment": "confirmed"
            }
        },
        "SOL-SOL": {
            "token": "SOL",
            "decimal_ratio": 1,
            "liquidity_amount": "500",
            "type": "svm_to_svm",
            "source": {
                "name": "Solana Mainnet",
                "chain_id": 0,
                "rpc_url": "https://api.mainnet-beta.solana.com",
                "token_address": "So11111111111111111111111111111111111111112",
                "token_decimals": 9,
                "commitment": "finalized"
            },
            "target": {
                "name": "1024 Chain",
                "chain_id": 1024,
                "rpc_url": "https://rpc.1024.chain",
                "token_address": "TargetAddr111111111111111111111111111111111",
                "token_decimals": 9,
                "commitment": "confirmed"
            }
        }
    }"#;

    #[test]
    fn test_parse_bridges_config() {
        let config = BridgesConfig::from_json(SAMPLE_JSON).unwrap();
        assert_eq!(config.bridges.len(), 2);
        assert!(config.bridges.contains_key("USDT"));
        assert!(config.bridges.contains_key("SOL-SOL"));
    }

    #[test]
    fn test_get_bridge() {
        let config = BridgesConfig::from_json(SAMPLE_JSON).unwrap();
        let usdt = config.get_bridge("USDT").unwrap();
        assert_eq!(usdt.token, "USDT");
        assert_eq!(usdt.decimal_ratio, 1_000_000_000_000);
        assert!(config.get_bridge("NONEXISTENT").is_none());
    }

    #[test]
    fn test_direction_explicit_type() {
        let config = BridgesConfig::from_json(SAMPLE_JSON).unwrap();
        let usdt = config.get_bridge("USDT").unwrap();
        assert_eq!(usdt.direction(), BridgeDirection::EvmToSvm);

        let sol = config.get_bridge("SOL-SOL").unwrap();
        assert_eq!(sol.direction(), BridgeDirection::SvmToSvm);
    }

    #[test]
    fn test_direction_legacy_fallback() {
        let json = r#"{
            "LEGACY": {
                "token": "ETH",
                "decimal_ratio": 1,
                "liquidity_amount": "100",
                "evm": {
                    "name": "Ethereum",
                    "chain_id": 1,
                    "rpc_url": "https://rpc",
                    "token_address": "0x0",
                    "token_decimals": 18
                },
                "svm": {
                    "name": "Solana",
                    "chain_id": 0,
                    "rpc_url": "https://rpc",
                    "token_address": "addr",
                    "token_decimals": 9
                }
            }
        }"#;
        let config = BridgesConfig::from_json(json).unwrap();
        let entry = config.get_bridge("LEGACY").unwrap();
        assert_eq!(entry.direction(), BridgeDirection::EvmToSvm);
    }

    #[test]
    fn test_source_target_config() {
        let config = BridgesConfig::from_json(SAMPLE_JSON).unwrap();

        let usdt = config.get_bridge("USDT").unwrap();
        let src = usdt.source_config().unwrap();
        assert_eq!(src.name, "Ethereum");
        let tgt = usdt.target_config().unwrap();
        assert_eq!(tgt.name, "Solana");

        let sol = config.get_bridge("SOL-SOL").unwrap();
        let src = sol.source_config().unwrap();
        assert_eq!(src.name, "Solana Mainnet");
        let tgt = sol.target_config().unwrap();
        assert_eq!(tgt.name, "1024 Chain");
    }

    #[test]
    fn test_chain_config_debug_redacts_rpc_url() {
        let config = BridgesConfig::from_json(SAMPLE_JSON).unwrap();
        let usdt = config.get_bridge("USDT").unwrap();
        let debug_str = format!("{:?}", usdt.source_config().unwrap());
        assert!(debug_str.contains("<redacted>"));
        assert!(!debug_str.contains("infura"));
        assert!(!debug_str.contains("secret"));
    }

    #[test]
    fn test_chain_config_optional_fields() {
        let config = BridgesConfig::from_json(SAMPLE_JSON).unwrap();
        let usdt = config.get_bridge("USDT").unwrap();

        let evm = usdt.source_config().unwrap();
        assert_eq!(evm.confirmation_blocks, Some(12));
        assert!(evm.commitment.is_none());

        let svm = usdt.target_config().unwrap();
        assert_eq!(svm.commitment.as_deref(), Some("confirmed"));
        assert!(svm.confirmation_blocks.is_none());
    }
}
