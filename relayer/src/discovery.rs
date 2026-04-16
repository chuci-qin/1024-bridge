//! 链上发现模块
//!
//! 从 1024 链的桥合约中读取：
//! - BridgeState PDA：获取 usdc_mint、local_chain_id、relayer 白名单等全局状态
//! - PeerConfig PDAs：通过 getProgramAccounts 发现所有已配置的对端链
//!
//! 这样 relayer 不需要任何静态配置文件，所有对端链信息都从链上动态获取。

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use solana_account_decoder::UiAccountEncoding;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use tracing::{info, warn};

use crate::chain_registry::{get_chain_info, resolve_rpc};
use crate::types::PeerInfo;

/// 从 BridgeState PDA 解析出的关键信息
#[derive(Debug)]
pub struct BridgeStateInfo {
    /// 1024 链自身的 chain_id
    pub local_chain_id: u64,
    /// USDC 代币的 mint 地址
    pub usdc_mint: Pubkey,
    /// 已授权的 relayer 公钥列表
    pub relayers: Vec<Pubkey>,
}

/// 计算 Anchor 账户的鉴别器（discriminator）。
/// Anchor 约定：SHA-256("account:{账户名}") 的前 8 字节。
fn anchor_discriminator(account_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(format!("account:{account_name}"));
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

/// 从 1024 链读取 BridgeState PDA 账户，解析出全局配置。
///
/// BridgeState 在 Anchor 中的内存布局（简化）：
/// ```text
/// [8B disc] [32B admin] [32B guardian] [32B operator] [32B recovery] [32B pending_admin]
/// [1B vault_bump] [32B usdc_mint] [8B local_chain_id] [56B 7×u64 rate limits]
/// [1B is_paused] [1B timelock_active] [4B relayer_count] [relayer_count × 32B relayers...]
/// ```
pub fn fetch_bridge_state(rpc: &RpcClient, program_id: &Pubkey) -> Result<BridgeStateInfo> {
    // 用 seeds=["bridge_state"] 派生 PDA 地址
    let (bridge_state_pda, _) = Pubkey::find_program_address(&[b"bridge_state"], program_id);

    let account = rpc
        .get_account_with_commitment(&bridge_state_pda, CommitmentConfig::finalized())?
        .value
        .with_context(|| "BridgeState PDA 账户不存在")?;

    let data = &account.data;
    let disc = anchor_discriminator("BridgeState");

    // 校验鉴别器（前 8 字节）
    if data.len() < 8 || data[..8] != disc {
        bail!("BridgeState 鉴别器不匹配");
    }

    // 跳过固定大小字段到达我们需要的位置
    // 布局：8(disc) + 32×5(admin/guardian/operator/recovery/pending_admin) + 1(vault_bump)
    let mut offset = 8 + 32 * 5 + 1;

    // 读取 usdc_mint（32 字节）
    let usdc_mint = Pubkey::try_from(&data[offset..offset + 32])
        .map_err(|e| anyhow::anyhow!("解析 usdc_mint 失败: {e:?}"))?;
    offset += 32;

    // 读取 local_chain_id（8 字节，小端序）
    let local_chain_id = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
    offset += 8;

    // 跳过 7 个 u64 速率限制字段（56 字节）
    offset += 8 * 7;
    // 跳过 is_paused(1) + timelock_active(1)
    offset += 2;

    // 读取 relayers 动态数组
    if offset + 4 > data.len() {
        bail!("BridgeState 数据太短，无法读取 relayer 数量");
    }
    let relayer_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    let mut relayers = Vec::with_capacity(relayer_count);
    for _ in 0..relayer_count {
        if offset + 32 > data.len() {
            bail!("BridgeState 数据太短，无法读取 relayer 公钥");
        }
        let pk = Pubkey::try_from(&data[offset..offset + 32])
            .map_err(|e| anyhow::anyhow!("解析 relayer pubkey 失败: {e:?}"))?;
        relayers.push(pk);
        offset += 32;
    }

    Ok(BridgeStateInfo {
        local_chain_id,
        usdc_mint,
        relayers,
    })
}

/// 通过 getProgramAccounts 发现所有 PeerConfig 账户。
///
/// PeerConfig 在 Anchor 中的内存布局：
/// ```text
/// [8B disc] [8B chain_id] [32B peer_contract]
/// ```
///
/// 使用 Memcmp 过滤器仅匹配 PeerConfig 鉴别器，避免获取其他类型的账户。
/// 对于每个发现的 peer，查询链注册表获取 RPC URL 和链类型。
pub fn discover_peers(rpc: &RpcClient, program_id: &Pubkey) -> Result<Vec<PeerInfo>> {
    let disc = anchor_discriminator("PeerConfig");

    // 按鉴别器前缀过滤，只获取 PeerConfig 类型的账户
    let filters = vec![RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
        0,
        disc.to_vec(),
    ))];

    let config = RpcProgramAccountsConfig {
        filters: Some(filters),
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            commitment: Some(CommitmentConfig::finalized()),
            ..Default::default()
        },
        ..Default::default()
    };

    let accounts = rpc.get_program_accounts_with_config(program_id, config)?;

    let mut peers = Vec::new();
    for (_pubkey, account) in &accounts {
        let data = &account.data;
        // 最少需要：8(disc) + 8(chain_id) + 32(peer_contract) = 48 字节
        if data.len() < 8 + 8 + 32 {
            warn!("跳过数据长度不足的 PeerConfig 账户");
            continue;
        }

        // 读取 chain_id（disc 后面的 8 字节）
        let chain_id = u64::from_le_bytes(data[8..16].try_into()?);
        // 读取 peer_contract（32 字节）
        let mut peer_contract = [0u8; 32];
        peer_contract.copy_from_slice(&data[16..48]);

        // 从链注册表查找该 chain_id 对应的信息（默认 RPC、链类型等）
        let chain_info = match get_chain_info(chain_id) {
            Some(ci) => ci,
            None => {
                warn!(chain_id, "chain_id 不在链注册表中，跳过该 peer");
                continue;
            }
        };

        // 解析 RPC URL（优先使用环境变量覆盖，否则用默认值）
        let rpc_url = resolve_rpc(chain_info);

        peers.push(PeerInfo {
            chain_id,
            peer_contract,
            kind: chain_info.kind,
            rpc_url,
        });

        info!(
            chain_id,
            kind = %chain_info.kind,
            env_name = chain_info.env_name,
            "发现 peer 链"
        );
    }

    Ok(peers)
}
