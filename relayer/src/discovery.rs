//! 链上发现模块
//!
//! 从 SVM 桥合约中读取：
//! - BridgeState PDA：获取 usdc_mint、local_chain_id、relayer 白名单等全局状态
//!   - **Hub**（`bridge1024_hub`，部署到 1024 chain）字段布局含全局速率限制 + relayers
//!   - **Leaf**（`bridge1024`，部署到 Solana 等叶子链）字段布局额外含 peer 配置
//! - PeerConfig PDAs：仅 hub 程序使用，通过 getProgramAccounts 发现所有已配置的对端链
//!
//! 这样 relayer 不需要任何静态配置文件，所有对端链信息都从 1024 hub 链动态获取。
//!
//! 反序列化策略（M5 修复 + hub/leaf 拆分）：
//! 不再硬编码字节偏移，而是用 borsh 镜像 struct 自动按字段顺序解析。
//! 字段顺序必须与
//! `contracts/svm/programs/bridge1024_hub/src/state.rs::BridgeState`（hub 形态）
//! 和 `contracts/svm/programs/bridge1024/src/state.rs::BridgeState`（leaf 形态）
//! 严格一致；任何顺序/类型变更会让 borsh 反序列化立即报错。

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
use crate::types::{PeerInfo, SvmProgramKind};

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
// ⚠️ 重要：以下 struct 的字段顺序、类型、个数必须严格与合约源码一致：
// - `HubBridgeStateData` ↔ contracts/svm/programs/bridge1024_hub/src/state.rs::BridgeState
// - `LeafBridgeStateData` ↔ contracts/svm/programs/bridge1024/src/state.rs::BridgeState
// - `PeerConfigData` ↔ contracts/svm/programs/bridge1024_hub/src/state.rs::PeerConfig
// 修改合约 struct 时务必同步修改这里。
// ─────────────────────────────────────────────────────────────────────────────

/// Hub 形态（`bridge1024_hub`）`BridgeState` 的 borsh 镜像
/// （去掉 8 字节 discriminator 之后的 payload）。
///
/// 多 Peer 版本：peer 相关配置在独立的 `PeerConfig` PDA 中，本结构体仅保留
/// 全局配置和安全机制（速率限制、relayers、角色等）。
///
/// Anchor 用 borsh 序列化账户数据，因此用 borsh `BorshDeserialize` 直接还原；
/// `Pubkey` 用 `[u8; 32]` 表示，避免依赖 solana-sdk 的 borsh feature。
///
/// 大量字段虽然 relayer 当前不需要（admin/guardian/各种 rate-limit 计数等），
/// 但**必须全部声明**且按合约里的顺序，否则 borsh 反序列化会读偏。
/// 因此整个 struct 标 `dead_code` 豁免；后续如果业务用到了某些字段，去掉即可。
#[derive(BorshDeserialize)]
#[allow(dead_code)]
struct HubBridgeStateData {
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

/// Leaf 形态（`bridge1024`）`BridgeState` 的 borsh 镜像
/// （去掉 8 字节 discriminator 之后的 payload）。
///
/// 单 Peer 版本：peer 配置（peer_chain_id / peer_contract / bridge_fee /
/// gasless_fee / max_stake_amount）直接内嵌在 BridgeState 中。
/// 字段顺序必须与 `contracts/svm/programs/bridge1024/src/state.rs::BridgeState` 一致。
///
/// 与 hub 形态的关键差异：在 `local_chain_id` 之后多了 5 个 peer 字段
/// （共 64 字节）；其余前后段（角色 / 速率限制 / 标志位 / relayers）顺序一致。
#[derive(BorshDeserialize)]
#[allow(dead_code)]
struct LeafBridgeStateData {
    admin: [u8; 32],
    guardian: [u8; 32],
    operator: [u8; 32],
    recovery: [u8; 32],
    pending_admin: [u8; 32],
    vault_bump: u8,
    usdc_mint: [u8; 32],
    local_chain_id: u64,
    // ── leaf 特有 ─────────────────────────────────────────────
    peer_chain_id: u64,
    peer_contract: [u8; 32],
    bridge_fee: u64,
    gasless_fee: u64,
    max_stake_amount: u64,
    // ── 速率限制（与 hub 完全相同的 7 个 u64） ───────────────
    max_unlock_per_window: u64,
    window_duration: u64,
    current_window_start: u64,
    current_window_usage: u64,
    previous_window_usage: u64,
    max_single_unlock: u64,
    minimum_reserve: u64,
    // ── 标志位 + 中继器 ──────────────────────────────────────
    is_paused: bool,
    timelock_active: bool,
    relayers: Vec<[u8; 32]>,
}

/// `PeerConfig` 的 borsh 镜像（去掉 8 字节 discriminator 之后的 payload）。
///
/// 仅 hub 程序拥有该账户类型。同 BridgeState 镜像：未使用字段必须按链上顺序声明。
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

/// 从某 SVM 链读取 BridgeState PDA 账户，解析出全局配置。
///
/// `kind` 决定使用 hub 还是 leaf 形态的字段布局：
/// - `Hub`（1024 chain，部署 `bridge1024_hub`）
/// - `Leaf`（Solana 等叶子链，部署 `bridge1024`）
///
/// 字段顺序必须与合约 `BridgeState` 一致。借助 borsh 反序列化，
/// 任何字段类型/顺序错位都会立即报错（而不是像之前硬编码偏移那样
/// 静默错位 → 解出错误的 usdc_mint → 后续所有 PDA/ATA 全错）。
///
/// 注意：borsh `deserialize` 不要求消耗所有 bytes —— 账户末尾可能有
/// realloc 预留的零字节，**这是预期行为**。
pub async fn fetch_bridge_state(
    rpc: &RpcClient,
    program_id: &Pubkey,
    kind: SvmProgramKind,
) -> Result<BridgeStateInfo> {
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

    decode_bridge_state(payload, kind)
}

/// 把已剥离 discriminator 的 BridgeState 字节按指定形态解析为 `BridgeStateInfo`。
///
/// 抽出来方便单测验证两种形态的字段解析正确性，不需要 mock RPC。
fn decode_bridge_state(payload: &[u8], kind: SvmProgramKind) -> Result<BridgeStateInfo> {
    let mut cursor: &[u8] = payload;
    match kind {
        SvmProgramKind::Hub => {
            let bs = HubBridgeStateData::deserialize(&mut cursor).context(
                "反序列化 hub BridgeState 失败（合约 struct 可能已变更，\
                 需同步更新 HubBridgeStateData）",
            )?;
            Ok(BridgeStateInfo {
                local_chain_id: bs.local_chain_id,
                usdc_mint: Pubkey::new_from_array(bs.usdc_mint),
                relayers: bs
                    .relayers
                    .into_iter()
                    .map(Pubkey::new_from_array)
                    .collect(),
            })
        }
        SvmProgramKind::Leaf => {
            let bs = LeafBridgeStateData::deserialize(&mut cursor).context(
                "反序列化 leaf BridgeState 失败（合约 struct 可能已变更，\
                 需同步更新 LeafBridgeStateData）",
            )?;
            Ok(BridgeStateInfo {
                local_chain_id: bs.local_chain_id,
                usdc_mint: Pubkey::new_from_array(bs.usdc_mint),
                relayers: bs
                    .relayers
                    .into_iter()
                    .map(Pubkey::new_from_array)
                    .collect(),
            })
        }
    }
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

    /// 按 hub 形态布局手工拼出 BridgeState payload（不含 8B discriminator）。
    ///
    /// 字段顺序必须严格对齐 `HubBridgeStateData` / 合约 `BridgeState`：
    /// 5 个 Pubkey（角色）+ vault_bump + usdc_mint + local_chain_id
    /// + 7 个 u64（速率限制）+ 2 个 bool（标志位）+ relayers Vec。
    fn build_hub_bridge_state_payload(
        usdc_mint: &[u8; 32],
        local_chain_id: u64,
        relayers: &[[u8; 32]],
    ) -> Vec<u8> {
        let mut data = Vec::new();
        // 5 个角色 Pubkey（用占位数据）
        for byte in [0x01u8, 0x02, 0x03, 0x04, 0x05] {
            data.extend_from_slice(&[byte; 32]);
        }
        data.push(254); // vault_bump
        data.extend_from_slice(usdc_mint);
        data.extend_from_slice(&local_chain_id.to_le_bytes());
        // 7 个速率限制 u64
        for _ in 0..7 {
            data.extend_from_slice(&0u64.to_le_bytes());
        }
        data.push(0); // is_paused
        data.push(1); // timelock_active
        // relayers: Vec<Pubkey>（4B len + entries）
        data.extend_from_slice(&(relayers.len() as u32).to_le_bytes());
        for r in relayers {
            data.extend_from_slice(r);
        }
        data
    }

    /// 按 leaf 形态布局手工拼出 BridgeState payload（不含 8B discriminator）。
    ///
    /// 与 hub 的差异：在 local_chain_id 之后多 5 个 peer 字段
    /// （peer_chain_id / peer_contract / bridge_fee / gasless_fee / max_stake_amount）。
    fn build_leaf_bridge_state_payload(
        usdc_mint: &[u8; 32],
        local_chain_id: u64,
        peer_chain_id: u64,
        peer_contract: &[u8; 32],
        relayers: &[[u8; 32]],
    ) -> Vec<u8> {
        let mut data = Vec::new();
        for byte in [0x11u8, 0x22, 0x33, 0x44, 0x55] {
            data.extend_from_slice(&[byte; 32]);
        }
        data.push(253); // vault_bump
        data.extend_from_slice(usdc_mint);
        data.extend_from_slice(&local_chain_id.to_le_bytes());
        // leaf 特有 peer 字段
        data.extend_from_slice(&peer_chain_id.to_le_bytes());
        data.extend_from_slice(peer_contract);
        data.extend_from_slice(&100u64.to_le_bytes()); // bridge_fee
        data.extend_from_slice(&50u64.to_le_bytes()); // gasless_fee
        data.extend_from_slice(&u64::MAX.to_le_bytes()); // max_stake_amount
        // 7 个速率限制 u64
        for _ in 0..7 {
            data.extend_from_slice(&0u64.to_le_bytes());
        }
        data.push(0); // is_paused
        data.push(0); // timelock_active
        data.extend_from_slice(&(relayers.len() as u32).to_le_bytes());
        for r in relayers {
            data.extend_from_slice(r);
        }
        data
    }

    /// hub 形态解析：能正确还原 usdc_mint / local_chain_id / relayers 三个核心字段。
    #[test]
    fn decode_hub_bridge_state_returns_correct_fields() {
        let mint = [0xAB; 32];
        let r1 = [0x77u8; 32];
        let r2 = [0x88u8; 32];
        let payload = build_hub_bridge_state_payload(&mint, 91024, &[r1, r2]);

        let info = decode_bridge_state(&payload, SvmProgramKind::Hub).expect("decode hub");
        assert_eq!(info.local_chain_id, 91024);
        assert_eq!(info.usdc_mint.to_bytes(), mint);
        assert_eq!(info.relayers.len(), 2);
        assert_eq!(info.relayers[0].to_bytes(), r1);
        assert_eq!(info.relayers[1].to_bytes(), r2);
    }

    /// leaf 形态解析：能正确还原 usdc_mint / local_chain_id / relayers，
    /// 中间多出来的 peer 字段被正确跳过（如果跳错位 relayers 长度会乱）。
    #[test]
    fn decode_leaf_bridge_state_returns_correct_fields() {
        let mint = [0xCD; 32];
        let peer = [0xEE; 32];
        let r1 = [0x99u8; 32];
        let payload = build_leaf_bridge_state_payload(&mint, 101, 91024, &peer, &[r1]);

        let info = decode_bridge_state(&payload, SvmProgramKind::Leaf).expect("decode leaf");
        assert_eq!(info.local_chain_id, 101);
        assert_eq!(info.usdc_mint.to_bytes(), mint);
        assert_eq!(info.relayers.len(), 1);
        assert_eq!(info.relayers[0].to_bytes(), r1);
    }

    /// 用 leaf 形态去解 hub 字节会失败：hub 在 local_chain_id 之后是速率限制
    /// 而不是 peer 字段，会把 relayers 长度位读到错的位置，触发巨长 Vec 分配失败。
    /// 反过来同理（用 hub 解 leaf 也会失败）。这是双形态分发的安全网。
    #[test]
    fn decode_with_wrong_kind_fails() {
        let mint = [0xAB; 32];
        let r1 = [0x77u8; 32];
        let hub_payload = build_hub_bridge_state_payload(&mint, 91024, &[r1]);
        // 用 leaf 解 hub：要么直接 Err，要么解出错误结果（relayers 长度乱套）
        let leaf_attempt = decode_bridge_state(&hub_payload, SvmProgramKind::Leaf);
        let ok_with_correct_data = leaf_attempt
            .map(|info| info.relayers.len() == 1 && info.local_chain_id == 91024)
            .unwrap_or(false);
        assert!(
            !ok_with_correct_data,
            "用 leaf 形态解 hub 字节绝不应正确还原（否则形态分发就失去意义）"
        );
    }
}
