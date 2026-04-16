//! 配置模块
//!
//! 从环境变量读取 relayer 运行所需的全部配置。
//! 主要环境变量：
//! - `BRIDGE_1024_PROGRAM_ID`：1024 链上桥合约的 Program ID
//! - `BRIDGE_1024_NETWORK`：网络类型（mainnet / stablenet / testnet），用于确定 chain_id 和 RPC
//! - `DATA_DIR`：数据持久化根目录（默认 /data）

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::chain_registry::{get_chain_info, network_to_chain_id, resolve_rpc};

/// Relayer 运行时的全部配置，由环境变量派生而来。
#[derive(Clone, Debug)]
pub struct Config {
    /// 1024 链上桥合约的 Program ID（base58 编码的 Pubkey）
    pub bridge_program_id: String,
    /// 网络名称：mainnet / stablenet / testnet
    pub network: String,
    /// 1024 链的 chain_id（由 network 映射而来，如 91024 = mainnet）
    pub chain_1024_id: u64,
    /// 1024 链的 RPC URL（优先读取环境变量 RPC_1024_xxx，否则用默认值）
    pub chain_1024_rpc: String,
    /// 数据持久化根目录，下面会有 keys/、checkpoints/、logs/ 三个子目录
    pub data_dir: PathBuf,
}

impl Config {
    /// 从环境变量构建配置。失败时返回可读的错误信息。
    pub fn from_env() -> Result<Self> {
        let bridge_program_id = env::var("BRIDGE_1024_PROGRAM_ID")
            .context("必须设置 BRIDGE_1024_PROGRAM_ID 环境变量")?;

        let network = env::var("BRIDGE_1024_NETWORK")
            .context("必须设置 BRIDGE_1024_NETWORK 环境变量（mainnet | stablenet | testnet）")?;

        // 将网络名称映射为 chain_id（mainnet=91024, testnet=91025, stablenet=91026）
        let chain_1024_id = network_to_chain_id(&network)
            .with_context(|| format!("未知的 BRIDGE_1024_NETWORK 值: {network}"))?;

        // 从链注册表中查找 1024 链的默认 RPC
        let chain_info = get_chain_info(chain_1024_id)
            .with_context(|| format!("链注册表中找不到 1024 chain_id {chain_1024_id}"))?;

        // 尝试读取 RPC_1024_xxx 环境变量覆盖，否则使用默认 RPC
        let chain_1024_rpc = resolve_rpc(chain_info);

        // 数据目录，Docker 环境通常挂载 volume 到 /data
        let data_dir = PathBuf::from(env::var("DATA_DIR").unwrap_or_else(|_| "/data".to_string()));

        Ok(Config {
            bridge_program_id,
            network,
            chain_1024_id,
            chain_1024_rpc,
            data_dir,
        })
    }

    /// 密钥存储目录：{data_dir}/keys/
    pub fn keys_dir(&self) -> PathBuf {
        self.data_dir.join("keys")
    }

    /// 检查点存储目录：{data_dir}/checkpoints/
    pub fn checkpoints_dir(&self) -> PathBuf {
        self.data_dir.join("checkpoints")
    }

    /// 日志存储目录：{data_dir}/logs/
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// 确保所有数据子目录存在，不存在则自动创建。
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [self.keys_dir(), self.checkpoints_dir(), self.logs_dir()] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("创建目录失败: {}", dir.display()))?;
        }
        Ok(())
    }
}
