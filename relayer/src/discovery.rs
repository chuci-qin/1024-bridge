//! 链上发现模块
//!
//! 从 1024 链的桥合约中读取：
//! - BridgeState PDA：获取 usdc_mint、local_chain_id、relayer 白名单等全局状态
//! - PeerConfig PDAs：通过 getProgramAccounts 发现所有已配置的对端链
//!
//! 这样 relayer 不需要任何静态配置文件，所有对端链信息都从链上动态获取。
//!
//! 反序列化策略（M5 修复）：
//! 不再硬编码字节偏移，而是用 borsh 镜像 struct 自动按字段顺序解析。
//! 字段顺序必须与 `contracts/svm/programs/bridge1024/src/state.rs` 中的
//! `BridgeState` / `PeerConfig` 严格一致；任何顺序/类型变更会让 borsh 反序列化
//! 立即报错，而不是像硬编码偏移那样静默错位。

use anyhow::{bail, Context, Result};
use borsh::BorshDeserialize;
use sha2::{Digest, Sha256};
use solana_account_decoder::UiAccountEncoding;
use solana_client::nonblocking::rpc_client::RpcClient;
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

// ─────────────────────────────────────────────────────────────────────────────
// 链上 struct 的 borsh 镜像
//
// ⚠️ 重要：以下 struct 的字段顺序、类型、个数必须严格与
// `contracts/svm/programs/bridge1024/src/state.rs` 一致。
// 修改合约 struct 时务必同步修改这里。
// ─────────────────────────────────────────────────────────────────────────────

/// `BridgeState` 的 borsh 镜像（去掉 8 字节 discriminator 之后的 payload）。
///
/// Anchor 用 borsh 序列化账户数据，因此用 borsh `BorshDeserialize` 直接还原；
/// `Pubkey` 用 `[u8; 32]` 表示，避免依赖 solana-sdk 的 borsh feature。
///
/// 大量字段虽然 relayer 当前不需要（admin/guardian/各种 rate-limit 计数等），
/// 但**必须全部声明**且按合约里的顺序，否则 borsh 反序列化会读偏。
/// 因此整个 struct 标 `dead_code` 豁免；后续如果业务用到了某些字段，去掉即可。
#[derive(BorshDeserialize)]
#[allow(dead_code)]
struct BridgeStateData {
    admin: [u8; 32],
    guardian: [u8; 32],
    operator: [u8; 32],
    recovery: [u8; 32],
    pending_admin: [u8; 32],
    vault_bump: u8,
    usdc_mint: [u8; 32],
    local_chain_id: u64,
    max_unlock_per_window: u64,
    window_duration: u64,
    current_window_start: u64,
    current_window_usage: u64,
    previous_window_usage: u64,
    max_single_unlock: u64,
    minimum_reserve: u64,
    is_paused: bool,
    timelock_active: bool,
    relayers: Vec<[u8; 32]>,
}

/// `PeerConfig` 的 borsh 镜像（去掉 8 字节 discriminator 之后的 payload）。
///
/// 同 `BridgeStateData`：未使用字段必须按链上顺序声明，整 struct 豁免 dead_code。
#[derive(BorshDeserialize)]
#[allow(dead_code)]
struct PeerConfigData {
    chain_id: u64,
    peer_contract: [u8; 32],
    bridge_fee: u64,
    max_stake_amount: u64,
    max_unlock_per_window: u64,
    window_duration: u64,
    max_single_unlock: u64,
    current_window_start: u64,
    current_window_usage: u64,
    previous_window_usage: u64,
}

/// 从账户数据校验并剥离 8 字节 discriminator，返回剩余 payload 切片。
fn strip_discriminator<'a>(
    data: &'a [u8],
    expected_disc: [u8; 8],
    account_name: &str,
) -> Result<&'a [u8]> {
    if data.len() < 8 {
        bail!("{account_name} 账户数据长度 {} < 8", data.len());
    }
    if data[..8] != expected_disc {
        bail!(
            "{account_name} 鉴别器不匹配: expected {}, got {}",
            hex::encode(expected_disc),
            hex::encode(&data[..8])
        );
    }
    Ok(&data[8..])
}

/// 从 1024 链读取 BridgeState PDA 账户，解析出全局配置。
///
/// 字段顺序必须与合约 `BridgeState` 一致。借助 borsh 反序列化，
/// 任何字段类型/顺序错位都会立即报错（而不是像之前硬编码偏移那样
/// 静默错位 → 解出错误的 usdc_mint → 后续所有 PDA/ATA 全错）。
///
/// 注意：`BridgeStateData` 反序列化时会消耗到 `relayers` Vec 末尾即停止；
/// 账户末尾可能有 realloc 预留的零字节，**这是预期行为**（`deserialize`
/// 不要求消耗所有 bytes）。
pub async fn fetch_bridge_state(rpc: &RpcClient, program_id: &Pubkey) -> Result<BridgeStateInfo> {
    let (bridge_state_pda, _) = Pubkey::find_program_address(&[b"bridge_state"], program_id);

    let account = rpc
        .get_account_with_commitment(&bridge_state_pda, CommitmentConfig::finalized())
        .await?
        .value
        .with_context(|| "BridgeState PDA 账户不存在")?;

    let payload = strip_discriminator(
        &account.data,
        anchor_discriminator("BridgeState"),
        "BridgeState",
    )?;

    // 用游标逐字节消费，允许末尾保留 realloc 预留的空间
    let mut cursor: &[u8] = payload;
    let bs = BridgeStateData::deserialize(&mut cursor)
        .context("反序列化 BridgeState 失败（合约 struct 可能已变更，需同步更新 BridgeStateData）")?;

    Ok(BridgeStateInfo {
        local_chain_id: bs.local_chain_id,
        usdc_mint: Pubkey::new_from_array(bs.usdc_mint),
        relayers: bs.relayers.into_iter().map(Pubkey::new_from_array).collect(),
    })
}

/// 通过 getProgramAccounts 发现所有 PeerConfig 账户。
///
/// 使用 Memcmp 过滤器仅匹配 PeerConfig 鉴别器，避免获取其他类型的账户。
/// 对于每个发现的 peer，查询链注册表获取 RPC URL 和链类型。
///
/// 与 `fetch_bridge_state` 一样，用 borsh 反序列化代替硬编码偏移。
pub async fn discover_peers(rpc: &RpcClient, program_id: &Pubkey) -> Result<Vec<PeerInfo>> {
    let disc = anchor_discriminator("PeerConfig");

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

    let accounts = rpc
        .get_program_accounts_with_config(program_id, config)
        .await?;

    let mut peers = Vec::new();
    for (pda, account) in &accounts {
        let payload = match strip_discriminator(&account.data, disc, "PeerConfig") {
            Ok(p) => p,
            Err(e) => {
                warn!(pda = %pda, "跳过 PeerConfig: {e}");
                continue;
            }
        };

        let mut cursor: &[u8] = payload;
        let pc = match PeerConfigData::deserialize(&mut cursor) {
            Ok(pc) => pc,
            Err(e) => {
                warn!(
                    pda = %pda,
                    "反序列化 PeerConfig 失败（合约 struct 可能已变更）: {e}"
                );
                continue;
            }
        };

        let chain_info = match get_chain_info(pc.chain_id) {
            Some(ci) => ci,
            None => {
                warn!(chain_id = pc.chain_id, "chain_id 不在链注册表中，跳过该 peer");
                continue;
            }
        };

        let rpc_url = resolve_rpc(chain_info);

        peers.push(PeerInfo {
            chain_id: pc.chain_id,
            peer_contract: pc.peer_contract,
            kind: chain_info.kind,
            rpc_url,
        });

        info!(
            chain_id = pc.chain_id,
            kind = %chain_info.kind,
            env_name = chain_info.env_name,
            "发现 peer 链"
        );
    }

    Ok(peers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// 鉴别器是 SHA256("account:{name}") 的前 8 字节，纯函数。
    /// 这条断言验证：函数实现 = 标准 Anchor 公式。
    #[test]
    fn anchor_discriminator_matches_sha256_prefix() {
        for name in ["BridgeState", "PeerConfig", "CrossChainRequest"] {
            let mut hasher = Sha256::new();
            hasher.update(format!("account:{name}"));
            let expected = &hasher.finalize()[..8];
            let got = anchor_discriminator(name);
            assert_eq!(&got[..], expected, "discriminator mismatch for {name}");
        }
    }

    /// 同一账户名两次调用必须得到相同结果（防止内部用了非确定性哈希）。
    #[test]
    fn anchor_discriminator_is_deterministic() {
        let a = anchor_discriminator("BridgeState");
        let b = anchor_discriminator("BridgeState");
        assert_eq!(a, b);
    }

    /// 不同账户名必须得到不同的鉴别器（碰撞率极低，但仍校验）。
    #[test]
    fn anchor_discriminator_differs_per_name() {
        let a = anchor_discriminator("BridgeState");
        let b = anchor_discriminator("PeerConfig");
        assert_ne!(a, b);
    }

    /// strip_discriminator 校验：长度不足 → Err；前 8B 不匹配 → Err。
    #[test]
    fn strip_discriminator_validates_input() {
        let disc = anchor_discriminator("BridgeState");

        // case 1: 数据太短
        assert!(strip_discriminator(&[0u8; 4], disc, "BridgeState").is_err());

        // case 2: 鉴别器不匹配
        let mut bad = vec![0u8; 16];
        bad[..8].copy_from_slice(&[0xff; 8]);
        assert!(strip_discriminator(&bad, disc, "BridgeState").is_err());

        // case 3: 正常数据 → 返回去掉 8B 头部的 payload
        let mut good = vec![0u8; 16];
        good[..8].copy_from_slice(&disc);
        good[8..].copy_from_slice(&[0xab; 8]);
        let payload = strip_discriminator(&good, disc, "BridgeState").expect("ok");
        assert_eq!(payload, &[0xab; 8]);
    }
}
