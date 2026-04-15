use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::chain_registry::{get_chain_info, network_to_chain_id, resolve_rpc};

/// All configuration needed by the relayer, derived from environment variables.
#[derive(Clone, Debug)]
pub struct Config {
    pub bridge_program_id: String,
    pub network: String,
    pub chain_1024_id: u64,
    pub chain_1024_rpc: String,
    pub data_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bridge_program_id = env::var("BRIDGE_1024_PROGRAM_ID")
            .context("BRIDGE_1024_PROGRAM_ID is required")?;

        let network = env::var("BRIDGE_1024_NETWORK")
            .context("BRIDGE_1024_NETWORK is required (mainnet | stablenet | testnet)")?;

        let chain_1024_id = network_to_chain_id(&network)
            .with_context(|| format!("Unknown BRIDGE_1024_NETWORK value: {network}"))?;

        let chain_info = get_chain_info(chain_1024_id)
            .with_context(|| format!("No chain_registry entry for 1024 chain_id {chain_1024_id}"))?;

        let chain_1024_rpc = resolve_rpc(chain_info);

        let data_dir = PathBuf::from(env::var("DATA_DIR").unwrap_or_else(|_| "/data".to_string()));

        Ok(Config {
            bridge_program_id,
            network,
            chain_1024_id,
            chain_1024_rpc,
            data_dir,
        })
    }

    pub fn keys_dir(&self) -> PathBuf {
        self.data_dir.join("keys")
    }

    pub fn checkpoints_dir(&self) -> PathBuf {
        self.data_dir.join("checkpoints")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [self.keys_dir(), self.checkpoints_dir(), self.logs_dir()] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
        }
        Ok(())
    }
}
