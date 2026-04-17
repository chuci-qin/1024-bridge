//! 检查点（Checkpoint）持久化模块
//!
//! 记录每条链 poller 的扫描进度，避免重启后重复扫描。新架构下每条链恰好
//! 一个 poller，故 checkpoint 文件只按 chain_id 区分，无方向前缀。
//!
//! - EVM 链：记录上次扫描到的 finalized 区块号
//! - SVM 链：记录上次扫描到的最新交易签名（signature）
//!
//! 文件路径格式：`{checkpoints_dir}/{chain_id}.json`
//! 例如：`/data/checkpoints/1.json`（Ethereum Mainnet）
//!      `/data/checkpoints/91024.json`（1024 链）
//!
//! 写入使用 tmp + rename 原子策略，避免半截文件。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// EVM 检查点：记录上次扫描到的区块号
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmCheckpoint {
    /// 下次扫描的起始区块号（inclusive）
    pub last_block: u64,
}

/// SVM 检查点：记录上次扫描到的最新交易签名
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SvmCheckpoint {
    /// 上次看到的最新交易签名（base58 编码），用作 getSignaturesForAddress 的 until 参数
    pub last_signature: String,
}

/// 构造 checkpoint 文件路径：`{dir}/{chain_id}.json`
fn checkpoint_path(dir: &Path, chain_id: u64) -> PathBuf {
    dir.join(format!("{chain_id}.json"))
}

/// 加载 EVM checkpoint。文件不存在返回 None（表示首次启动）。
pub fn load_evm_checkpoint(dir: &Path, chain_id: u64) -> Result<Option<EvmCheckpoint>> {
    let path = checkpoint_path(dir, chain_id);
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("读取 checkpoint 失败: {}", path.display()))?;
    let cp: EvmCheckpoint = serde_json::from_str(&data)
        .with_context(|| format!("解析 checkpoint 失败: {}", path.display()))?;
    Ok(Some(cp))
}

/// 保存 EVM checkpoint。使用先写临时文件再 rename 的原子写入策略。
pub fn save_evm_checkpoint(dir: &Path, chain_id: u64, cp: &EvmCheckpoint) -> Result<()> {
    let path = checkpoint_path(dir, chain_id);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cp)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 加载 SVM checkpoint。文件不存在返回 None（表示首次启动）。
pub fn load_svm_checkpoint(dir: &Path, chain_id: u64) -> Result<Option<SvmCheckpoint>> {
    let path = checkpoint_path(dir, chain_id);
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("读取 checkpoint 失败: {}", path.display()))?;
    let cp: SvmCheckpoint = serde_json::from_str(&data)
        .with_context(|| format!("解析 checkpoint 失败: {}", path.display()))?;
    Ok(Some(cp))
}

/// 保存 SVM checkpoint。使用先写临时文件再 rename 的原子写入策略。
pub fn save_svm_checkpoint(dir: &Path, chain_id: u64, cp: &SvmCheckpoint) -> Result<()> {
    let path = checkpoint_path(dir, chain_id);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cp)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
