use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::types::Direction;

/// EVM checkpoint: tracks last scanned block.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmCheckpoint {
    pub last_block: u64,
}

/// SVM checkpoint: tracks last seen transaction signature.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SvmCheckpoint {
    pub last_signature: String,
}

fn checkpoint_path(dir: &Path, direction: Direction, chain_id: u64) -> PathBuf {
    dir.join(format!("{}_{}.json", direction, chain_id))
}

pub fn load_evm_checkpoint(dir: &Path, direction: Direction, chain_id: u64) -> Result<Option<EvmCheckpoint>> {
    let path = checkpoint_path(dir, direction, chain_id);
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("read checkpoint {}", path.display()))?;
    let cp: EvmCheckpoint = serde_json::from_str(&data)
        .with_context(|| format!("parse checkpoint {}", path.display()))?;
    Ok(Some(cp))
}

pub fn save_evm_checkpoint(dir: &Path, direction: Direction, chain_id: u64, cp: &EvmCheckpoint) -> Result<()> {
    let path = checkpoint_path(dir, direction, chain_id);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cp)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn load_svm_checkpoint(dir: &Path, direction: Direction, chain_id: u64) -> Result<Option<SvmCheckpoint>> {
    let path = checkpoint_path(dir, direction, chain_id);
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("read checkpoint {}", path.display()))?;
    let cp: SvmCheckpoint = serde_json::from_str(&data)
        .with_context(|| format!("parse checkpoint {}", path.display()))?;
    Ok(Some(cp))
}

pub fn save_svm_checkpoint(dir: &Path, direction: Direction, chain_id: u64, cp: &SvmCheckpoint) -> Result<()> {
    let path = checkpoint_path(dir, direction, chain_id);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cp)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
