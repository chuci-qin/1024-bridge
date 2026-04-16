//! Bridge1024 Relayer 主入口
//!
//! 整体架构：
//! - 启动时从 1024 链上读取 BridgeState 和所有 PeerConfig，自动发现需要监听的对端链
//! - 为每个 peer 启动一个 Inbound task（监听 peer 链 → 在 1024 链上确认）
//! - 启动一个全局 Outbound poller（监听 1024 链 → 在对应 peer 链上确认）
//! - 每个事件独立 tokio::spawn 并行处理，失败的进入 retry 队列下轮重试

mod chain_registry;
mod checkpoint;
mod config;
mod discovery;
mod error;
mod evm;
mod keys;
mod logging;
mod svm;
mod types;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ethers::providers::{Http, Provider};
use ethers::signers::LocalWallet;
use ethers::types::Address;
use rand::Rng;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::signer::Signer;
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::checkpoint::*;
use crate::config::Config;
use crate::types::*;

/// 事件处理任务的 JoinHandle 类型别名。
/// 返回 (事件数据, 是否成功)，成功=true 表示已处理或已跳过，false 表示需要重试。
type EvtHandle = tokio::task::JoinHandle<(StakeEventData, bool)>;

/// 将一个事件的确认提交到 SVM 目标链的异步任务。
/// 需要 owned 数据因为要跨 tokio::spawn 边界。
fn spawn_svm_confirm(
    rpc: Arc<RpcClient>,
    program_id: Pubkey,
    usdc_mint: Pubkey,
    token_program_id: Pubkey,
    kp_bytes: [u8; 64],       // Keypair 不能 Clone，通过 bytes 传递后在 spawn 内重建
    event: StakeEventData,
    peer_chain_id: u64,
    direction: Direction,
) -> EvtHandle {
    tokio::spawn(async move {
        let kp = solana_sdk::signature::Keypair::try_from(kp_bytes.as_slice()).expect("keypair");
        let ok = process_event_for_svm(
            &rpc, &program_id, &usdc_mint, &token_program_id, &kp,
            &event, peer_chain_id, direction,
        ).await;
        (event, ok)
    })
}

/// 将一个事件的确认提交到 EVM 目标链的异步任务。
/// Provider<Http> 和 LocalWallet 都可以 Clone，直接传入。
fn spawn_evm_confirm(
    wallet: LocalWallet,
    provider: Provider<Http>,
    contract_address: Address,
    chain_id: u64,
    event: StakeEventData,
    direction: Direction,
) -> EvtHandle {
    tokio::spawn(async move {
        let ok = process_event_for_evm(
            &wallet, &provider, contract_address,
            chain_id, &event, direction,
        ).await;
        (event, ok)
    })
}

/// 等待所有 spawn 的事件处理任务完成，收集失败的事件（用于放回 retry 队列）。
async fn collect_failures(handles: Vec<EvtHandle>) -> Vec<StakeEventData> {
    let mut failed = Vec::new();
    for r in futures::future::join_all(handles).await {
        match r {
            Ok((_, true)) => {}                          // 成功处理，无需操作
            Ok((event, false)) => failed.push(event),    // 处理失败，需要重试
            Err(e) => warn!("事件处理任务 panic: {e}"),
        }
    }
    failed
}

/// 正常轮询间隔（秒）
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// 追赶模式下的轮询间隔（毫秒），用于快速处理积压区块
const CATCHUP_DELAY: Duration = Duration::from_millis(200);
/// EVM 每次 eth_getLogs 的最大区块范围（Alchemy 免费版上限 10 个区块）
const EVM_BLOCK_RANGE: u64 = 10;
/// EVM 首次启动时向前扫描的区块数（从 finalized 区块往回扫 1000 块）
const EVM_INITIAL_SCAN_BACK: u64 = 1000;
/// SVM 每次 getSignaturesForAddress 的分页大小
const SVM_SIG_BATCH: usize = 50;
/// SVM 单轮 poll 最多累计获取的 signature 数量
const SVM_MAX_SIGS: usize = 1000;

#[tokio::main]
async fn main() -> Result<()> {
    // ── 1. 加载配置 ──────────────────────────────────────────────────
    let config = Config::from_env()?;
    config.ensure_dirs()?;  // 确保 keys/, checkpoints/, logs/ 目录存在

    // ── 2. 初始化日志（JSON 格式，同时输出到 stderr 和文件）─────────
    logging::init(&config.logs_dir())?;

    info!(
        network = %config.network,
        chain_id = config.chain_1024_id,
        rpc = %config.chain_1024_rpc,
        "启动 Bridge1024 relayer"
    );

    // ── 3. 加载或自动生成密钥对 ─────────────────────────────────────
    // 首次启动会自动生成 SVM Keypair 和 EVM 私钥，保存到 /data/keys/
    // 同时输出 addresses.json 和 WARN 日志，提醒运维去合约上白名单
    let keys = keys::Keys::load_or_generate(&config.keys_dir())?;
    let svm_pubkey = keys.svm_keypair.pubkey();

    info!(svm_pubkey = %svm_pubkey, "Relayer 密钥已加载");

    // ── 4. 连接 1024 链，读取链上状态 ───────────────────────────────
    let program_id = Pubkey::from_str(&config.bridge_program_id)
        .context("BRIDGE_1024_PROGRAM_ID 格式无效")?;

    let rpc_1024 = RpcClient::new_with_commitment(
        config.chain_1024_rpc.clone(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );

    // 4a. 读取 BridgeState PDA —— 获取 usdc_mint、relayer 白名单等
    info!("正在从 1024 链读取 BridgeState...");
    let bridge_state = discovery::fetch_bridge_state(&rpc_1024, &program_id)?;

    info!(
        local_chain_id = bridge_state.local_chain_id,
        relayer_count = bridge_state.relayers.len(),
        "BridgeState 加载完成"
    );

    // 检查自己的公钥是否在白名单中
    if !bridge_state.relayers.contains(&svm_pubkey) {
        warn!("本机 SVM 公钥不在桥合约 relayer 白名单中 —— 确认交易会失败，请先在合约上添加白名单");
    }

    // 4b. 发现所有 PeerConfig —— 确定需要监听哪些对端链
    info!("正在发现 peer 配置...");
    let peers = discovery::discover_peers(&rpc_1024, &program_id)?;

    if peers.is_empty() {
        bail!("未发现任何 peer 配置 —— 没有可 relay 的链");
    }

    info!(peer_count = peers.len(), "Peer 发现完成");

    // 4c. 检测 USDC 所属的 Token Program（SPL Token 还是 Token-2022）
    let usdc_mint = bridge_state.usdc_mint;
    let token_program_id = {
        let mint_account = rpc_1024
            .get_account(&usdc_mint)
            .context("读取 USDC mint 账户以检测 token program")?;
        info!(
            usdc_mint = %usdc_mint,
            token_program = %mint_account.owner,
            "检测到 USDC token program"
        );
        mint_account.owner
    };

    // ── 5. 启动并行任务 ─────────────────────────────────────────────
    let config = Arc::new(config);
    let mut handles = Vec::new();

    // 5a. 为每个 peer 启动一个 Inbound task（peer 链 → 1024 链）
    // 每个 task 独立轮询自己的 peer 链，发现 StakeEvent 后在 1024 链上提交 confirm
    for peer in &peers {
        let peer = peer.clone();
        let config = Arc::clone(&config);
        let program_id = program_id;
        let rpc_url_1024 = config.chain_1024_rpc.clone();
        let usdc_mint = usdc_mint;
        let token_program_id = token_program_id;
        let svm_keypair_bytes = keys.svm_keypair.to_bytes().to_vec();

        let handle = tokio::spawn(async move {
            let keypair = solana_sdk::signature::Keypair::try_from(svm_keypair_bytes.as_slice())
                .expect("重建 keypair");
            if let Err(e) = run_inbound_task(
                &config,
                &peer,
                &program_id,
                &rpc_url_1024,
                &usdc_mint,
                &token_program_id,
                &keypair,
            )
            .await
            {
                error!(
                    chain_id = peer.chain_id,
                    direction = "inbound",
                    "Inbound 任务失败: {e:#}"
                );
            }
        });
        handles.push(handle);
    }

    // 5b. 启动全局唯一的 Outbound poller（1024 链 → 所有 peer 链）
    // 只轮询 1024 链一次，按 target_chain_id 分发到不同 peer
    {
        let config = Arc::clone(&config);
        let peers = peers.clone();
        let program_id = program_id;
        let rpc_url_1024 = config.chain_1024_rpc.clone();
        let usdc_mint = usdc_mint;
        let token_program_id = token_program_id;
        let evm_wallet = keys.evm_wallet.clone();
        let svm_keypair_bytes = keys.svm_keypair.to_bytes().to_vec();

        let handle = tokio::spawn(async move {
            let keypair = solana_sdk::signature::Keypair::try_from(svm_keypair_bytes.as_slice())
                .expect("重建 keypair");
            if let Err(e) = run_outbound_poller(
                &config,
                &peers,
                &program_id,
                &rpc_url_1024,
                &usdc_mint,
                &token_program_id,
                &evm_wallet,
                &keypair,
            )
            .await
            {
                error!("Outbound poller 失败: {e:#}");
            }
        });
        handles.push(handle);
    }

    // 所有 task 都是无限循环，join_all 永远不会返回（除非 task panic）
    futures::future::join_all(handles).await;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════
// Inbound 逻辑：监听 peer 链 → 在 1024 链上提交 confirm_event
// ═══════════════════════════════════════════════════════════════════════

/// Inbound 入口：根据 peer 链类型（EVM/SVM）分派到具体实现。
async fn run_inbound_task(
    config: &Config,
    peer: &PeerInfo,
    program_id: &Pubkey,
    rpc_url_1024: &str,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    relayer_keypair: &solana_sdk::signature::Keypair,
) -> Result<()> {
    let checkpoints_dir = config.checkpoints_dir();

    info!(
        chain_id = peer.chain_id,
        kind = %peer.kind,
        direction = "inbound",
        "启动 Inbound poller"
    );

    match peer.kind {
        ChainKind::Evm => {
            run_inbound_evm(
                &checkpoints_dir, peer, program_id, rpc_url_1024,
                usdc_mint, token_program_id, relayer_keypair,
            ).await
        }
        ChainKind::Svm => {
            run_inbound_svm(
                &checkpoints_dir, peer, program_id, rpc_url_1024,
                usdc_mint, token_program_id, relayer_keypair,
            ).await
        }
    }
}

/// Inbound EVM：轮询 EVM peer 链上的 StakeEvent，在 1024 链（SVM）上提交确认。
///
/// 流程：
/// 1. 从 checkpoint 恢复上次扫描位置（区块号），没有则从 finalized-1000 开始
/// 2. 每轮循环：
///    a. 过滤 retry 队列中已被其他 relayer 处理的 nonce
///    b. spawn 所有 retry + 新 event 的确认任务（并行）
///    c. join_all 等待完成，失败的放回 retry
///    d. 保存 checkpoint
///    e. sleep（追赶模式 200ms，正常 5s）
async fn run_inbound_evm(
    checkpoints_dir: &std::path::Path,
    peer: &PeerInfo,
    program_id: &Pubkey,
    rpc_url_1024: &str,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    relayer_keypair: &solana_sdk::signature::Keypair,
) -> Result<()> {
    // 创建 EVM provider 连接 peer 链
    let provider = Provider::<Http>::try_from(&peer.rpc_url)
        .context("创建 EVM provider")?;
    // 创建 SVM RPC client 连接 1024 链（用 Arc 以便跨 spawn 共享）
    let target_rpc = Arc::new(RpcClient::new_with_commitment(
        rpc_url_1024.to_string(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    ));
    // Keypair 不能 Clone，提取 bytes 后在每个 spawn 内重建
    let kp_bytes = relayer_keypair.to_bytes();

    // 将 peer_contract（bytes32）转换为 EVM Address（取后 20 字节）
    let contract_address = bytes32_to_evm_address(&peer.peer_contract)?;

    // 恢复 checkpoint，或首次启动时从 finalized-1000 块开始扫描
    let mut from_block = match load_evm_checkpoint(checkpoints_dir, Direction::Inbound, peer.chain_id)? {
        Some(cp) => cp.last_block,
        None => {
            let start = evm::poller::initial_from_block(&provider, EVM_INITIAL_SCAN_BACK).await?;
            info!(chain_id = peer.chain_id, from_block = start, "无 checkpoint，从最近区块开始扫描");
            start
        }
    };

    // 失败事件的重试队列（内存中，重启会丢失，但 checkpoint 不会推进所以不会漏）
    let mut pending_retry: Vec<StakeEventData> = Vec::new();

    loop {
        let mut catching_up = false;
        let mut handles: Vec<EvtHandle> = Vec::new();

        // ── 步骤 1：过滤 retry 队列 ──
        // 同步调用 check_nonce_processed，已被其他 relayer 处理的直接丢弃
        pending_retry.retain(|event| {
            match svm::submitter::check_nonce_processed(&target_rpc, program_id, event.source_chain_id, event.nonce) {
                Ok(true) => false,  // 已处理，移除
                _ => true,          // 未处理或查询失败，保留
            }
        });

        // ── 步骤 2：spawn retry 事件（并行）──
        for event in pending_retry.drain(..) {
            handles.push(spawn_svm_confirm(
                Arc::clone(&target_rpc), *program_id, *usdc_mint, *token_program_id,
                kp_bytes, event, peer.chain_id, Direction::Inbound,
            ));
        }

        // ── 步骤 3：轮询新事件并 spawn（并行）──
        // eth_getLogs 只查询 finalized 区块，避免 reorg 导致误处理
        match evm::poller::poll_evm_events(&provider, contract_address, from_block, EVM_BLOCK_RANGE).await {
            Ok((events, new_from)) => {
                for event in events {
                    handles.push(spawn_svm_confirm(
                        Arc::clone(&target_rpc), *program_id, *usdc_mint, *token_program_id,
                        kp_bytes, event, peer.chain_id, Direction::Inbound,
                    ));
                }

                // 推进 checkpoint（只在 poll 成功时推进）
                if new_from > from_block {
                    // 如果一次跨了多个区块范围，说明在追赶，用更短的 sleep
                    catching_up = (new_from - from_block) > EVM_BLOCK_RANGE;
                    from_block = new_from;
                    let cp = EvmCheckpoint { last_block: from_block };
                    if let Err(e) = save_evm_checkpoint(checkpoints_dir, Direction::Inbound, peer.chain_id, &cp) {
                        warn!(chain_id = peer.chain_id, "保存 checkpoint 失败: {e}");
                    }
                }
            }
            Err(e) => {
                // poll 失败时 from_block 不变，下轮重试同一区间，不会漏事件
                warn!(chain_id = peer.chain_id, "EVM poll 错误: {e}");
            }
        }

        // ── 步骤 4：等待所有并行任务完成，收集失败的放回 retry ──
        pending_retry.extend(collect_failures(handles).await);

        // ── 步骤 5：休眠 ──
        if catching_up {
            sleep(CATCHUP_DELAY).await;   // 追赶模式：200ms
        } else {
            sleep(POLL_INTERVAL).await;   // 正常模式：5s
        }
    }
}

/// Inbound SVM：轮询 SVM peer 链上的 StakeEvent，在 1024 链（SVM）上提交确认。
///
/// 与 EVM 版本的区别：
/// - 用 getSignaturesForAddress 分页获取交易签名（而非 eth_getLogs）
/// - 从每笔交易的日志中解析 Anchor 格式的 StakeEvent
/// - checkpoint 记录的是 signature 而非区块号
async fn run_inbound_svm(
    checkpoints_dir: &std::path::Path,
    peer: &PeerInfo,
    program_id: &Pubkey,
    rpc_url_1024: &str,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    relayer_keypair: &solana_sdk::signature::Keypair,
) -> Result<()> {
    // 连接 peer SVM 链（用于轮询事件）
    let peer_rpc = RpcClient::new_with_commitment(
        peer.rpc_url.clone(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );
    // 连接 1024 链（用于提交确认）
    let target_rpc = Arc::new(RpcClient::new_with_commitment(
        rpc_url_1024.to_string(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    ));
    let kp_bytes = relayer_keypair.to_bytes();

    // peer 链上桥合约的 Program ID
    let peer_program_id = Pubkey::new_from_array(peer.peer_contract);

    // 恢复 checkpoint（上次扫到的最新 signature），没有则从头扫描最近 1000 个
    let mut last_sig = match load_svm_checkpoint(checkpoints_dir, Direction::Inbound, peer.chain_id)? {
        Some(cp) => {
            Some(Signature::from_str(&cp.last_signature).context("解析已保存的 signature")?)
        }
        None => {
            info!(chain_id = peer.chain_id, max_sigs = SVM_MAX_SIGS, "无 checkpoint，扫描最近的 signatures");
            None
        }
    };

    let mut pending_retry: Vec<StakeEventData> = Vec::new();

    loop {
        let mut handles: Vec<EvtHandle> = Vec::new();

        // 过滤 retry 队列中已处理的 nonce
        pending_retry.retain(|event| {
            match svm::submitter::check_nonce_processed(&target_rpc, program_id, event.source_chain_id, event.nonce) {
                Ok(true) => false,
                _ => true,
            }
        });
        // spawn retry 事件
        for event in pending_retry.drain(..) {
            handles.push(spawn_svm_confirm(
                Arc::clone(&target_rpc), *program_id, *usdc_mint, *token_program_id,
                kp_bytes, event, peer.chain_id, Direction::Inbound,
            ));
        }

        // 轮询 peer SVM 链上的 StakeEvent
        // poll_svm_events 内部会分页获取 signatures，逐笔获取交易日志并解析事件
        match svm::poller::poll_svm_events(&peer_rpc, &peer_program_id, last_sig.as_ref(), SVM_SIG_BATCH, SVM_MAX_SIGS) {
            Ok((events, newest_sig)) => {
                // spawn 新事件
                for event in events {
                    handles.push(spawn_svm_confirm(
                        Arc::clone(&target_rpc), *program_id, *usdc_mint, *token_program_id,
                        kp_bytes, event, peer.chain_id, Direction::Inbound,
                    ));
                }

                // 推进 checkpoint
                if let Some(sig) = newest_sig {
                    last_sig = Some(sig);
                    let cp = SvmCheckpoint {
                        last_signature: sig.to_string(),
                    };
                    if let Err(e) = save_svm_checkpoint(checkpoints_dir, Direction::Inbound, peer.chain_id, &cp) {
                        warn!(chain_id = peer.chain_id, "保存 checkpoint 失败: {e}");
                    }
                }
            }
            Err(e) => {
                // poll 失败时 last_sig 不变，下轮从同一位置重试
                warn!(chain_id = peer.chain_id, "SVM poll 错误: {e}");
            }
        }

        // 收集失败事件放回 retry
        pending_retry.extend(collect_failures(handles).await);

        sleep(POLL_INTERVAL).await;
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Outbound 逻辑：监听 1024 链 → 在对应 peer 链上提交确认
// ═══════════════════════════════════════════════════════════════════════

/// Outbound 每个 peer 的上下文，包含连接资源和 retry 队列。
struct OutboundPeerCtx {
    peer: PeerInfo,
    /// EVM peer 的 HTTP provider（SVM peer 为 None）
    evm_provider: Option<Provider<Http>>,
    /// EVM peer 的合约地址（SVM peer 为 None）
    evm_address: Option<Address>,
    /// SVM peer 的 RPC 客户端（EVM peer 为 None）
    svm_rpc: Option<Arc<RpcClient>>,
    /// 提交失败的事件，下轮重试
    pending_retry: Vec<StakeEventData>,
}

impl OutboundPeerCtx {
    /// 根据 peer 链类型（EVM/SVM）spawn 对应的确认任务。
    fn spawn_confirm(
        &self,
        evm_wallet: &LocalWallet,
        usdc_mint: &Pubkey,
        token_program_id: &Pubkey,
        kp_bytes: [u8; 64],
        event: StakeEventData,
    ) -> EvtHandle {
        match self.peer.kind {
            ChainKind::Evm => spawn_evm_confirm(
                evm_wallet.clone(),
                self.evm_provider.clone().expect("EVM peer 必须有 provider"),
                self.evm_address.expect("EVM peer 必须有合约地址"),
                self.peer.chain_id,
                event,
                Direction::Outbound,
            ),
            ChainKind::Svm => spawn_svm_confirm(
                Arc::clone(self.svm_rpc.as_ref().expect("SVM peer 必须有 RPC")),
                Pubkey::new_from_array(self.peer.peer_contract),
                *usdc_mint,
                *token_program_id,
                kp_bytes,
                event,
                self.peer.chain_id,
                Direction::Outbound,
            ),
        }
    }
}

/// 统一 Outbound poller：轮询 1024 链一次，将事件分发到所有 peer 链并行确认。
///
/// 相比每个 peer 各自轮询 1024 链，减少了 N-1 次重复的 RPC 调用。
async fn run_outbound_poller(
    config: &Config,
    peers: &[PeerInfo],
    program_id: &Pubkey,
    rpc_url_1024: &str,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    evm_wallet: &LocalWallet,
    svm_keypair: &solana_sdk::signature::Keypair,
) -> Result<()> {
    let checkpoints_dir = config.checkpoints_dir();
    let kp_bytes = svm_keypair.to_bytes();

    // 连接 1024 链用于轮询事件
    let rpc_1024 = RpcClient::new_with_commitment(
        rpc_url_1024.to_string(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );

    // 为每个 peer 构建上下文（包含连接资源和 retry 队列）
    let mut peer_ctxs: HashMap<u64, OutboundPeerCtx> = HashMap::new();
    for peer in peers {
        let evm_provider = if peer.kind == ChainKind::Evm {
            Some(Provider::<Http>::try_from(&peer.rpc_url).context("创建 peer EVM provider")?)
        } else {
            None
        };
        let evm_address = if peer.kind == ChainKind::Evm {
            Some(bytes32_to_evm_address(&peer.peer_contract)?)
        } else {
            None
        };
        let svm_rpc = if peer.kind == ChainKind::Svm {
            Some(Arc::new(RpcClient::new_with_commitment(
                peer.rpc_url.clone(),
                solana_sdk::commitment_config::CommitmentConfig::finalized(),
            )))
        } else {
            None
        };

        info!(
            chain_id = peer.chain_id,
            kind = %peer.kind,
            direction = "outbound",
            "注册 outbound peer"
        );

        peer_ctxs.insert(peer.chain_id, OutboundPeerCtx {
            peer: peer.clone(),
            evm_provider,
            evm_address,
            svm_rpc,
            pending_retry: Vec::new(),
        });
    }

    // Outbound 使用 chain_id=0 作为统一 checkpoint 的标识
    const OUTBOUND_CHECKPOINT_ID: u64 = 0;

    // 恢复 outbound checkpoint
    let mut last_sig = match load_svm_checkpoint(&checkpoints_dir, Direction::Outbound, OUTBOUND_CHECKPOINT_ID)? {
        Some(cp) => {
            Some(Signature::from_str(&cp.last_signature).context("解析已保存的 outbound signature")?)
        }
        None => {
            info!(max_sigs = SVM_MAX_SIGS, "无 outbound checkpoint，扫描最近的 signatures");
            None
        }
    };

    info!(peer_count = peer_ctxs.len(), "启动统一 outbound poller");

    loop {
        let mut handles: Vec<EvtHandle> = Vec::new();

        // ── 步骤 1：spawn 所有 peer 的 retry 事件 ──
        for ctx in peer_ctxs.values_mut() {
            let retries: Vec<_> = ctx.pending_retry.drain(..).collect();
            for event in retries {
                handles.push(ctx.spawn_confirm(evm_wallet, usdc_mint, token_program_id, kp_bytes, event));
            }
        }

        // ── 步骤 2：轮询 1024 链一次，按 target_chain_id 分发 ──
        match svm::poller::poll_svm_events(&rpc_1024, program_id, last_sig.as_ref(), SVM_SIG_BATCH, SVM_MAX_SIGS) {
            Ok((events, newest_sig)) => {
                for event in events {
                    // 按事件的目标链 ID 查找对应的 peer 上下文
                    if let Some(ctx) = peer_ctxs.get(&event.target_chain_id) {
                        handles.push(ctx.spawn_confirm(evm_wallet, usdc_mint, token_program_id, kp_bytes, event));
                    } else {
                        warn!(
                            target_chain_id = event.target_chain_id,
                            nonce = event.nonce,
                            "目标链未注册，跳过该事件"
                        );
                    }
                }

                // 推进 checkpoint
                if let Some(sig) = newest_sig {
                    last_sig = Some(sig);
                    let cp = SvmCheckpoint {
                        last_signature: sig.to_string(),
                    };
                    if let Err(e) = save_svm_checkpoint(&checkpoints_dir, Direction::Outbound, OUTBOUND_CHECKPOINT_ID, &cp) {
                        warn!("保存 outbound checkpoint 失败: {e}");
                    }
                }
            }
            Err(e) => {
                // poll 失败时 last_sig 不变，下轮从同一位置重试
                warn!("Outbound poll 错误: {e}");
            }
        }

        // ── 步骤 3：收集失败事件，按 target_chain_id 放回各 peer 的 retry 队列 ──
        for event in collect_failures(handles).await {
            if let Some(ctx) = peer_ctxs.get_mut(&event.target_chain_id) {
                ctx.pending_retry.push(event);
            }
        }

        sleep(POLL_INTERVAL).await;
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 单个事件的处理逻辑（在 tokio::spawn 内执行）
// ═══════════════════════════════════════════════════════════════════════

/// 处理一个事件：在 SVM 目标链上提交 confirm_event。
///
/// 返回 true 表示已处理（成功提交或 nonce 已被处理），false 表示需要重试。
///
/// 流程：
/// 1. 随机 sleep 0~999ms（多个 relayer 实例错开提交，减少竞争）
/// 2. 查询链上 CrossChainRequest PDA 的 is_processed 字段
/// 3. 如果未处理，构建并发送 confirm_event 交易
async fn process_event_for_svm(
    rpc: &RpcClient,
    program_id: &Pubkey,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    relayer_keypair: &solana_sdk::signature::Keypair,
    event: &StakeEventData,
    peer_chain_id: u64,
    direction: Direction,
) -> bool {
    // 随机延迟，避免多个 relayer 同时提交导致冲突
    let delay_ms = rand::thread_rng().gen_range(0..1000);
    sleep(Duration::from_millis(delay_ms)).await;

    // 先检查 nonce 是否已被处理（可能被其他 relayer 先确认了）
    match svm::submitter::check_nonce_processed(rpc, program_id, event.source_chain_id, event.nonce) {
        Ok(true) => {
            info!(
                nonce = event.nonce,
                peer_chain_id,
                direction = %direction,
                "Nonce 已在 SVM 上处理，跳过"
            );
            return true;
        }
        Ok(false) => {} // 未处理，继续提交
        Err(e) => {
            warn!(
                nonce = event.nonce,
                peer_chain_id,
                direction = %direction,
                "查询 SVM nonce 状态失败: {e}"
            );
            return false; // 查询失败，放入 retry
        }
    }

    // 构建并发送 confirm_event 交易
    match svm::submitter::submit_confirm_event(rpc, program_id, relayer_keypair, usdc_mint, token_program_id, event) {
        Ok(sig) => {
            info!(
                nonce = event.nonce,
                peer_chain_id,
                direction = %direction,
                tx = %sig,
                "成功提交 SVM confirm_event"
            );
            true
        }
        Err(e) => {
            warn!(
                nonce = event.nonce,
                peer_chain_id,
                direction = %direction,
                "提交 SVM confirm_event 失败: {e}"
            );
            false // 提交失败，放入 retry
        }
    }
}

/// 处理一个事件：在 EVM 目标链上提交 confirmEvent。
///
/// 返回 true 表示已处理，false 表示需要重试。
/// 流程与 SVM 版本类似：随机延迟 → 检查 nonce → 提交交易。
async fn process_event_for_evm(
    evm_wallet: &LocalWallet,
    provider: &Provider<Http>,
    contract_address: Address,
    chain_id: u64,
    event: &StakeEventData,
    direction: Direction,
) -> bool {
    // 随机延迟
    let delay_ms = rand::thread_rng().gen_range(0..1000);
    sleep(Duration::from_millis(delay_ms)).await;

    // 检查 nonce 是否已处理（调用合约的 nonceConfirmations(nonce) 视图函数）
    match evm::submitter::check_nonce_processed(provider, contract_address, event.nonce).await {
        Ok(true) => {
            info!(
                nonce = event.nonce,
                chain_id,
                direction = %direction,
                "Nonce 已在 EVM 上处理，跳过"
            );
            return true;
        }
        Ok(false) => {}
        Err(e) => {
            warn!(
                nonce = event.nonce,
                chain_id,
                direction = %direction,
                "查询 EVM nonce 状态失败: {e}"
            );
            return false;
        }
    }

    // 构建并发送 confirmEvent 交易
    match evm::submitter::submit_confirm_event(evm_wallet, provider, contract_address, chain_id, event).await {
        Ok(tx_hash) => {
            info!(
                nonce = event.nonce,
                chain_id,
                direction = %direction,
                tx_hash = ?tx_hash,
                "成功提交 EVM confirmEvent"
            );
            true
        }
        Err(e) => {
            warn!(
                nonce = event.nonce,
                chain_id,
                direction = %direction,
                "提交 EVM confirmEvent 失败: {e}"
            );
            false
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 工具函数
// ═══════════════════════════════════════════════════════════════════════

/// 将 32 字节的 peer_contract 转换为 EVM Address（取后 20 字节）。
/// EVM 地址只有 20 字节，在 bytes32 中右对齐（前 12 字节为零填充）。
fn bytes32_to_evm_address(bytes32: &[u8; 32]) -> Result<Address> {
    Ok(Address::from_slice(&bytes32[12..]))
}
