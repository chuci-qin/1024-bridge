use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardianConfig {
    pub guardian: GuardianSettings,
    pub chains: ChainSettings,
    #[serde(default)]
    pub p2p: P2PSettings,
    #[serde(default)]
    pub api: ApiSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardianSettings {
    /// Guardian index (0-18 for 19-node setup)
    pub index: u8,
    
    /// Keystore settings
    pub keystore: KeystoreSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeystoreSettings {
    /// Path to encrypted keystore file
    pub path: String,
    
    /// Environment variable name for password
    pub password_env: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainSettings {
    pub evm: EvmChainConfig,
    pub solana: SolanaChainConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmChainConfig {
    /// WebSocket RPC URL
    pub rpc_url: String,
    
    /// Core contract address
    pub core_contract: String,
    
    /// Number of confirmations to wait
    #[serde(default = "default_evm_confirmations")]
    pub confirmations: u64,
}

fn default_evm_confirmations() -> u64 {
    64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaChainConfig {
    /// HTTP RPC URL
    pub rpc_url: String,
    
    /// WebSocket URL
    pub ws_url: String,
    
    /// Core program ID
    pub core_program: String,
    
    /// Commitment level
    #[serde(default = "default_commitment")]
    pub commitment: String,
}

fn default_commitment() -> String {
    "confirmed".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PSettings {
    /// Listen address
    #[serde(default = "default_p2p_listen")]
    pub listen_addr: String,
    
    /// Bootstrap peers
    #[serde(default)]
    pub bootstrap_peers: Vec<String>,
}

impl Default for P2PSettings {
    fn default() -> Self {
        Self {
            listen_addr: default_p2p_listen(),
            bootstrap_peers: vec![],
        }
    }
}

fn default_p2p_listen() -> String {
    "/ip4/0.0.0.0/tcp/4001".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSettings {
    /// Enable API server
    #[serde(default = "default_true")]
    pub enabled: bool,
    
    /// Listen address
    #[serde(default = "default_api_listen")]
    pub listen: String,
}

impl Default for ApiSettings {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            listen: default_api_listen(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_api_listen() -> String {
    "0.0.0.0:7071".to_string()
}

impl GuardianConfig {
    /// Load configuration from TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())
            .context("Failed to read configuration file")?;
        
        let config: GuardianConfig = toml::from_str(&content)
            .context("Failed to parse configuration")?;
        
        config.validate()?;
        
        Ok(config)
    }
    
    /// Validate configuration
    fn validate(&self) -> Result<()> {
        // Validate guardian index
        if self.guardian.index >= 19 {
            anyhow::bail!("Guardian index must be 0-18");
        }
        
        // Validate URLs
        if self.chains.evm.rpc_url.is_empty() {
            anyhow::bail!("EVM RPC URL is required");
        }
        
        if self.chains.solana.rpc_url.is_empty() {
            anyhow::bail!("Solana RPC URL is required");
        }
        
        Ok(())
    }
}

