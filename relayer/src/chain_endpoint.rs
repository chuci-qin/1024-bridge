//! 链端点（Endpoint）抽象
//!
//! 把 1024 链与所有 peer 链统一表达成 `ChainEndpoint`，让 main.rs 的 spawn
//! 逻辑可以对所有链一视同仁，不再需要"inbound vs outbound" / "1024 vs peer" 的分支。
//!
//! 关键设计：
//! - SVM 链额外携带 `SvmConfig { usdc_mint, token_program }`，因为 Solana 账户
//!   模型要求外部传入完整账户列表，而 ATA 派生需要 mint 和 token_program。
//!   EVM 没有这个问题：合约自己读 storage 拿 USDC 地址。
//! - 启动期 `build_all_endpoints` 会尝试从每条 SVM 链各自的 BridgeState 拉取
//!   `usdc_mint`，**1024 必须成功**（discovery 已经保证）；其它 SVM peer 失败
//!   仅 warn 不 bail，submitter 在运行时 lazy retry（与 EVM peer 启动期"只
//!   构造 URL 不真连"的容错风格一致）。

use std::collections::HashSet;
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use tracing::{info, warn};

use crate::chain_registry;
use crate::config::Config;
use crate::discovery::{fetch_bridge_state, BridgeStateInfo};
use crate::types::{ChainKind, PeerInfo, SvmProgramKind};

/// 一条链在 relayer 视角下的全部运行时信息。
///
/// `kind == Evm` → svm 永远 None；`kind == Svm` → svm 可能 None（启动期没拿到）。
#[derive(Clone, Debug)]
pub struct ChainEndpoint {
    pub chain_id: u64,
    pub kind: ChainKind,
    pub rpc_url: String,
    /// EVM：bytes32 中后 20B 是合约地址；SVM：直接当 Pubkey 用（program_id）。
    pub contract: [u8; 32],
    /// SVM 链才有；EVM 链恒为 None。
    pub svm: Option<SvmConfig>,
}

/// SVM 链提交 confirm_event 所需的运行时元数据。
///
/// `usdc_mint` 来自该链 BridgeState；`token_program` 是 mint 账户的 owner
/// （SPL Token 或 Token-2022 之一）；`program_kind` 决定 confirm_event 指令
/// 编码、CrossChainRequest PDA seeds、BridgeState 字段布局走 hub 还是 leaf 分支。
#[derive(Clone, Debug)]
pub struct SvmConfig {
    pub usdc_mint: Pubkey,
    pub token_program: Pubkey,
    pub program_kind: SvmProgramKind,
}

/// 从某 SVM 链上拉取 BridgeState，并推导 token_program。
///
/// `kind` 决定按 hub 还是 leaf 形态解析 BridgeState 字段布局；
/// 1024 chain 永远是 Hub，Solana 等叶子链永远是 Leaf。
///
/// 失败原因可能是：RPC 不通 / 桥合约未部署 / 合约 struct 已变更 / kind 传错。
/// 错误信息已 wrap context，调用方可直接 warn / bail。
pub async fn fetch_svm_config(
    rpc: &RpcClient,
    program_id: &Pubkey,
    kind: SvmProgramKind,
) -> Result<SvmConfig> {
    let bs = fetch_bridge_state(rpc, program_id, kind)
        .await
        .with_context(|| format!("拉取 BridgeState 失败 (program_id={program_id}, kind={kind})"))?;
    let mint_account = rpc
        .get_account(&bs.usdc_mint)
        .await
        .with_context(|| format!("读取 USDC mint 账户失败 (mint={})", bs.usdc_mint))?;
    Ok(SvmConfig {
        usdc_mint: bs.usdc_mint,
        token_program: mint_account.owner,
        program_kind: kind,
    })
}

/// 构造完整的链端点列表：1024 + 所有 peer。
///
/// - 1024 endpoint：复用 discovery 阶段已经拉到的 `BridgeStateInfo`（避免重复 RPC），
///   再追加一次 `get_account(usdc_mint)` 推导 token_program。
/// - 每个 EVM peer：直接构造 endpoint，svm = None。
/// - 每个 SVM peer：尝试连其自身 RPC + fetch_svm_config；失败仅 warn 跳过设
///   svm = None，不阻塞整个 relayer 启动。submitter 后续 lazy retry。
pub async fn build_all_endpoints(
    config: &Config,
    bridge_state: &BridgeStateInfo,
    rpc_1024: &RpcClient,
    peers: &[PeerInfo],
) -> Result<Vec<ChainEndpoint>> {
    let mut endpoints = Vec::with_capacity(1 + peers.len());

    // ── 1024 endpoint（永远是 hub 形态）────────────────────────────────
    let program_id = Pubkey::from_str(&config.bridge_program_id)
        .context("BRIDGE_1024_PROGRAM_ID 格式无效")?;
    let mint_account = rpc_1024
        .get_account(&bridge_state.usdc_mint)
        .await
        .context("读取 1024 USDC mint 账户")?;
    let svm_1024 = SvmConfig {
        usdc_mint: bridge_state.usdc_mint,
        token_program: mint_account.owner,
        program_kind: SvmProgramKind::Hub,
    };
    info!(
        chain_id = config.chain_1024_id,
        usdc_mint = %svm_1024.usdc_mint,
        token_program = %svm_1024.token_program,
        program_kind = %svm_1024.program_kind,
        "1024 endpoint 已就绪"
    );
    endpoints.push(ChainEndpoint {
        chain_id: config.chain_1024_id,
        kind: ChainKind::Svm,
        rpc_url: config.chain_1024_rpc.clone(),
        contract: program_id.to_bytes(),
        svm: Some(svm_1024),
    });

    // ── 每个 peer endpoint ────────────────────────────────────────────
    for peer in peers {
        let svm = match peer.kind {
            ChainKind::Evm => None,
            ChainKind::Svm => {
                // 从链注册表查程序形态：Solana → Leaf，其它 1024 网络 → Hub。
                // 未注册（实际不可能，因为 discovery 已经按注册表过滤过）→ warn 跳过 SvmConfig，
                // submitter 后续 lazy retry 时还会再走一次注册表查询。
                let Some(kind) = chain_registry::svm_program_kind(peer.chain_id) else {
                    warn!(
                        chain_id = peer.chain_id,
                        "SVM peer 在注册表中无 svm_program_kind 配置，跳过启动期 \
                         SvmConfig 拉取；submitter 将 lazy retry"
                    );
                    endpoints.push(ChainEndpoint {
                        chain_id: peer.chain_id,
                        kind: peer.kind,
                        rpc_url: peer.rpc_url.clone(),
                        contract: peer.peer_contract,
                        svm: None,
                    });
                    continue;
                };
                // 连 peer 自己的 RPC 拉它自己的 BridgeState（确保用对方的 USDC mint，
                // 而不是 1024 的——这是修掉的隐含 bug）。
                let peer_rpc = RpcClient::new_with_commitment(
                    peer.rpc_url.clone(),
                    CommitmentConfig::finalized(),
                );
                let peer_program_id = Pubkey::new_from_array(peer.peer_contract);
                match fetch_svm_config(&peer_rpc, &peer_program_id, kind).await {
                    Ok(cfg) => {
                        info!(
                            chain_id = peer.chain_id,
                            usdc_mint = %cfg.usdc_mint,
                            token_program = %cfg.token_program,
                            program_kind = %cfg.program_kind,
                            "SVM peer endpoint 已就绪"
                        );
                        Some(cfg)
                    }
                    Err(e) => {
                        warn!(
                            chain_id = peer.chain_id,
                            program_kind = %kind,
                            "SVM peer 启动期未取到 BridgeState（submitter 将 lazy retry）: {e:#}"
                        );
                        None
                    }
                }
            }
        };

        endpoints.push(ChainEndpoint {
            chain_id: peer.chain_id,
            kind: peer.kind,
            rpc_url: peer.rpc_url.clone(),
            contract: peer.peer_contract,
            svm,
        });
    }

    // 严防 chain_id 重复：每条链都要 spawn 自己的 poller + submitter，且 events/{chain_id}/
    // 与 checkpoints/{chain_id}.json 都用 chain_id 做唯一索引；如果两条 endpoint 撞 id：
    //   - 两个 poller 互相覆盖 checkpoint → 重复处理 / 漏事件
    //   - 两个 submitter 抢同一目录 → 同事件被多次广播
    // 链上 BridgeState 已经做了去重，但配置文件 / discovery 来源拼接后仍可能撞，启动期 fail-fast。
    let mut seen: HashSet<u64> = HashSet::with_capacity(endpoints.len());
    for ep in &endpoints {
        if !seen.insert(ep.chain_id) {
            bail!(
                "endpoint 列表中存在重复 chain_id={}（1024 配置与 peer 列表撞 id 或 peer 内部重复）",
                ep.chain_id
            );
        }
    }

    Ok(endpoints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evm_endpoint_has_no_svm_config() {
        let ep = ChainEndpoint {
            chain_id: 1,
            kind: ChainKind::Evm,
            rpc_url: "https://example".into(),
            contract: [0u8; 32],
            svm: None,
        };
        assert!(matches!(ep.kind, ChainKind::Evm));
        assert!(ep.svm.is_none());
    }

    /// 直接构造端点列表，验证去重逻辑能在两条 endpoint 撞同一个 chain_id 时报错。
    /// 不走 build_all_endpoints 完整路径（那需要真实 RPC），只复用同样的 HashSet 校验。
    #[test]
    fn duplicate_chain_id_detection_logic_holds() {
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let chain_ids = [91024_u64, 1, 1]; // 第二个 1 是重复
        let mut had_dup = false;
        for id in chain_ids {
            if !seen.insert(id) {
                had_dup = true;
            }
        }
        assert!(had_dup, "重复 chain_id 必须能被 HashSet::insert 检出");
    }

    #[test]
    fn svm_endpoint_carries_mint_and_token_program() {
        let cfg = SvmConfig {
            usdc_mint: Pubkey::new_unique(),
            token_program: Pubkey::new_unique(),
            program_kind: SvmProgramKind::Hub,
        };
        let ep = ChainEndpoint {
            chain_id: 91024,
            kind: ChainKind::Svm,
            rpc_url: "https://rpc".into(),
            contract: [1u8; 32],
            svm: Some(cfg.clone()),
        };
        let svm = ep.svm.expect("svm config");
        assert_eq!(svm.usdc_mint, cfg.usdc_mint);
        assert_eq!(svm.token_program, cfg.token_program);
        assert_eq!(svm.program_kind, SvmProgramKind::Hub);
    }

    #[test]
    fn svm_config_program_kind_round_trips() {
        let hub = SvmConfig {
            usdc_mint: Pubkey::new_unique(),
            token_program: Pubkey::new_unique(),
            program_kind: SvmProgramKind::Hub,
        };
        let leaf = SvmConfig {
            usdc_mint: Pubkey::new_unique(),
            token_program: Pubkey::new_unique(),
            program_kind: SvmProgramKind::Leaf,
        };
        assert_ne!(hub.program_kind, leaf.program_kind);
        assert_eq!(hub.program_kind, SvmProgramKind::Hub);
        assert_eq!(leaf.program_kind, SvmProgramKind::Leaf);
    }
}
