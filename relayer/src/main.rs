//! Bridge1024 Relayer 主入口
//!
//! 整体架构（解耦后）：
//! - 启动期从 1024 链上读取 BridgeState 和所有 PeerConfig，构造统一的
//!   `ChainEndpoint` 列表（含 1024 自己），SVM 链额外携带 (usdc_mint, token_program)。
//! - 为**每条链**（含 1024）spawn task，EVM 与 SVM 略有不同：
//!   - **EVM**（2 task）：poller + submitter
//!   - **SVM**（3 task）：sig enumerator + event extractor + submitter
//!     - Task A（sig enumerator）：`getSignaturesForAddress` → 空文件写入 `sigs/{chain_id}/` → 推进 checkpoint
//!     - Task B（event extractor）：读 `sigs/` → `getTransaction` 提取事件 → 写 `events/` → 删 sig 文件；
//!       超过 N 次失败的 sig 移入 `sigs_dead/`（DLQ），`error!` 告警
//!     - Task C（submitter）：与 EVM submitter 同理，不变
//! - 所有 task 完全独立、通过文件系统解耦。
//! - 1024 与其它链走完全相同代码路径，无 inbound/outbound 概念。

mod chain_endpoint;
mod chain_registry;
mod checkpoint;
mod config;
mod discovery;
mod evm;
mod keys;
mod logging;
mod pending_events;
mod svm;
mod types;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::Address;
use rand::seq::SliceRandom;
use rand::Rng;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::signer::Signer as SvmSigner;
use tracing::{error, info, warn};

use crate::chain_endpoint::{fetch_svm_config, ChainEndpoint, SvmConfig};
use crate::evm::submitter::EvmClient;
use crate::checkpoint::*;
use crate::config::Config;
use crate::pending_events::{
    delete_pending_event, load_all_pending_events, now_unix, save_pending_event,
    update_pending_entry, PendingEntry, Submission,
};
use crate::types::*;

// ─────────────────────────────────────────────────────────────────────────────
// 全局常量
// ─────────────────────────────────────────────────────────────────────────────

/// Poller 正常轮询间隔
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Poller 追赶模式下的轮询间隔（积压区块时快速消化）
const CATCHUP_DELAY: Duration = Duration::from_millis(200);
/// EVM 每次 eth_getLogs 的最大区块范围（Alchemy 免费版上限 10 个区块）
const EVM_BLOCK_RANGE: u64 = 10;
/// SVM 每次 getSignaturesForAddress 的分页大小
const SVM_SIG_BATCH: usize = 50;
/// SVM catch-up 提示阈值：某轮枚举到的签名数 ≥ 此值，说明刚清了一大批积压
/// （多半是长停机重启），下一轮改用 `CATCHUP_DELAY` 立刻再拉，尽快追平链头。
///
/// 注意：这**不是**总量截断上限。`enumerate_new_signatures` 每轮都会一路翻页
/// 到 checkpoint、返回全部新签名（见其文档），不会因为超过此值而丢弃任何签名。
const SVM_MAX_SIGS: usize = 1000;

/// EVM poller 是否要进入 catchup 模式。
///
/// `poll_evm_events` 返回的 `new_from = min(from + EVM_BLOCK_RANGE - 1, safe_head) + 1`，
/// 因此 `delta = new_from - from_block` 的上界恰好是 `EVM_BLOCK_RANGE`：
/// - delta == EVM_BLOCK_RANGE → 本轮拿满整页，safe_head 还在更远处，要继续追
/// - delta < EVM_BLOCK_RANGE → 已经追到 safe_head，按 POLL_INTERVAL 等新块
///
/// 历史 bug：曾用 `>` 而不是 `>=`，因 delta 永远不会超过 EVM_BLOCK_RANGE，
/// catchup 恒为 false → 长停机后追块只有 2 blocks/s 的速度。
fn evm_should_catch_up(from_block: u64, new_from: u64) -> bool {
    new_from.saturating_sub(from_block) >= EVM_BLOCK_RANGE
}

/// SVM sig enumerator 是否要进入 catchup 模式。
///
/// 本轮返回签名数 ≥ `SVM_MAX_SIGS` 说明刚清掉一大批积压，很可能在这轮（可能
/// 持续多次 RPC 翻页）期间链头又前进了不少，故下一轮不等 `POLL_INTERVAL`，
/// 改用 `CATCHUP_DELAY` 立刻再拉一轮以尽快追平。
fn svm_enumerator_should_catch_up(fetched: usize) -> bool {
    fetched >= SVM_MAX_SIGS
}
/// Submitter 每轮 sleep 的最小毫秒数
const SUBMIT_INTERVAL_MIN_MS: u64 = 1000;
/// Submitter 每轮 sleep 的最大毫秒数（jitter 上界）
const SUBMIT_INTERVAL_MAX_MS: u64 = 5000;

/// EVM stale 阈值的兜底默认（10 min）。
///
/// 正常路径是 `chain_registry::stale_pending_tx_secs(chain_id)` 按链分档取值：
/// - L1（ETH/Sepolia）600s
/// - L2（Arbitrum*）60s、Base* 120s
///
/// 这里的常量只在极端情况下用到：比如运维误把未注册 chain_id 接进来、
/// 且该链竟然还有 pending event 要处理（正常启动就会被别处 bail）。
const STALE_PENDING_TX_SECS_FALLBACK: u64 = 600;

/// SVM event extractor 对同一 sig 最多尝试多少次 `getTransaction`。
/// 超过此阈值后 sig 被移入 DLQ (sigs_dead/)，error! 一行便于告警。
const SVM_EXTRACT_MAX_ATTEMPTS: u32 = 10;
/// SVM event extractor 同一 sig 两次 attempt 之间的最小间隔（秒）。
/// DLQ 触发的最早时间 = (MAX_ATTEMPTS - 1) × MIN_RETRY_INTERVAL = 270s，
/// 天然给短时 RPC 抖动留够窗口，不再单独维护 MIN_LIFETIME 常量。
const SVM_EXTRACT_MIN_RETRY_INTERVAL_SECS: u64 = 30;

/// SVM submitter 的 lazy `fetch_svm_config` 每多少次连续失败升级一次 `error!`。
///
/// 选 30：在 1-2s jitter 周期下 ≈ 1 分钟出一条 error，既不刷屏又能触发监控告警。
/// 中间的失败仍以 `warn!` 记录，便于追溯首次失败时间。
const SVM_LAZY_FETCH_ERROR_EVERY: u32 = 30;

/// SVM 一笔已广播但始终查不到 status 的 tx，多久后视为"丢失"并允许重发。
/// 选 60s：与 `get_signature_statuses` 的 ~150 slot GC 窗口对齐——
/// 在此期间 status 仍可追踪；超过后 blockhash 也基本过期，tx 不可能再 land。
/// 重发由 Branch A `check_nonce_status` 兜底，preflight 会拦下 AlreadyProcessed。
const STALE_PENDING_SVM_TX_SECS: u64 = 60;

/// 生成 [SUBMIT_INTERVAL_MIN_MS, SUBMIT_INTERVAL_MAX_MS] 之间的随机间隔。
///
/// 多 relayer 实例共部署时错峰调用 RPC、错峰投票，避免同一时刻撞合约
/// 阈值票（让某条 tx 上链时其它 relayer 的 tx revert 浪费 gas）。
fn jittered_submit_interval() -> Duration {
    let ms = rand::thread_rng().gen_range(SUBMIT_INTERVAL_MIN_MS..=SUBMIT_INTERVAL_MAX_MS);
    Duration::from_millis(ms)
}

// ─────────────────────────────────────────────────────────────────────────────
// Graceful shutdown 工具（M9）
// ─────────────────────────────────────────────────────────────────────────────

type Shutdown = tokio::sync::watch::Receiver<bool>;

fn is_shutdown_requested(s: &Shutdown) -> bool {
    *s.borrow()
}

/// 等待固定时长，或在 shutdown 信号触发时立即返回。
/// 返回 true 表示是因 shutdown 触发提前返回。
async fn sleep_or_shutdown(dur: Duration, s: &mut Shutdown) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(dur) => false,
        _ = s.changed() => true,
    }
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).expect("无法注册 SIGTERM 处理器");
    let mut sigint = signal(SignalKind::interrupt()).expect("无法注册 SIGINT 处理器");
    tokio::select! {
        _ = sigterm.recv() => info!("收到 SIGTERM，开始优雅退出"),
        _ = sigint.recv() => info!("收到 SIGINT (Ctrl+C)，开始优雅退出"),
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("收到 Ctrl+C，开始优雅退出");
}

// ─────────────────────────────────────────────────────────────────────────────
// main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // ── 1. 配置 + 日志 + 密钥 ────────────────────────────────────────
    let config = Config::from_env()?;
    config.ensure_dirs()?;
    logging::init(&config.logs_dir())?;

    info!(
        network = %config.network,
        chain_id = config.chain_1024_id,
        rpc = %config.chain_1024_rpc,
        "启动 Bridge1024 relayer"
    );

    let keys = keys::Keys::load_or_generate(&config.keys_dir())?;
    let svm_pubkey = keys.svm_keypair.pubkey();
    info!(svm_pubkey = %svm_pubkey, "Relayer 密钥已加载");

    // ── 2. 1024 链 RPC 与 BridgeState 发现 ─────────────────────────────
    let program_id = Pubkey::from_str(&config.bridge_program_id)
        .context("BRIDGE_1024_PROGRAM_ID 格式无效")?;

    let rpc_1024 = RpcClient::new_with_commitment(
        config.chain_1024_rpc.clone(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );

    info!("正在从 1024 链读取 BridgeState（hub 形态）...");
    let bridge_state =
        discovery::fetch_bridge_state(&rpc_1024, &program_id, SvmProgramKind::Hub).await?;
    info!(
        local_chain_id = bridge_state.local_chain_id,
        relayer_count = bridge_state.relayers.len(),
        "BridgeState 加载完成"
    );

    // M6：链上 local_chain_id 必须与配置 chain_1024_id 一致
    if bridge_state.local_chain_id != config.chain_1024_id {
        bail!(
            "BridgeState.local_chain_id ({}) 与配置 chain_1024_id ({}) 不一致；\
             检查 BRIDGE_1024_NETWORK 与 BRIDGE_1024_PROGRAM_ID 是否匹配同一个网络",
            bridge_state.local_chain_id,
            config.chain_1024_id
        );
    }

    // 注意：1024 hub 的白名单状态此时其实已经能从 bridge_state.relayers 直接看出来，
    // 但为了让所有链的"是否已加白名单"由同一段逻辑统一打印（避免日志风格不一致 +
    // 漏掉 peer 链），统一推迟到 endpoints 构造完之后由 verify_relayer_whitelist 处理。

    info!("正在发现 peer 配置...");
    let peers = discovery::discover_peers(&rpc_1024, &program_id).await?;
    if peers.is_empty() {
        bail!("未发现任何 peer 配置 —— 没有可 relay 的链");
    }
    info!(peer_count = peers.len(), "Peer 发现完成");

    // ── 3. 构造统一的 endpoint 列表（1024 + 所有 peer）─────────────────
    let endpoints =
        chain_endpoint::build_all_endpoints(&config, &bridge_state, &rpc_1024, &peers).await?;

    let known_chain_ids: HashSet<u64> = endpoints.iter().map(|e| e.chain_id).collect();
    let known = Arc::new(known_chain_ids);

    info!(
        chains = endpoints.len(),
        ids = ?endpoints.iter().map(|e| e.chain_id).collect::<Vec<_>>(),
        "全部端点已就绪"
    );

    // 启动期对每条链做一次 relayer 白名单核查 —— 已注册 INFO 一行；
    // 没注册才 WARN 并指明 chain_id，避免每次启动都无差别 warn。
    verify_relayer_whitelist(&endpoints, &svm_pubkey, keys.evm_wallet.address()).await;

    // ── 4. shutdown channel + signal handler ──────────────────────────
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    {
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            wait_for_shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        });
    }

    // ── 5. 为每条链 spawn task ────────────────────────────────────────
    // EVM: 1 poller + 1 submitter = 2 task
    // SVM: 1 sig enumerator + 1 event extractor + 1 submitter = 3 task
    let config = Arc::new(config);
    let events_root = Arc::new(config.events_dir());
    let checkpoints_dir = Arc::new(config.checkpoints_dir());
    let sigs_dir = Arc::new(config.sigs_dir());
    let sigs_dead_dir = Arc::new(config.sigs_dead_dir());
    let mut handles = Vec::with_capacity(endpoints.len() * 3);

    for ep in &endpoints {
        match ep.kind {
            ChainKind::Evm => {
                // ── EVM poller ──
                {
                    let ep = ep.clone();
                    let events_root = Arc::clone(&events_root);
                    let checkpoints_dir = Arc::clone(&checkpoints_dir);
                    let known = Arc::clone(&known);
                    let shutdown = shutdown_rx.clone();
                    let shutdown_tx = shutdown_tx.clone();
                    handles.push(tokio::spawn(async move {
                        let chain_id = ep.chain_id;
                        if let Err(e) = run_evm_poller(ep, &events_root, &checkpoints_dir, &known, shutdown).await {
                            error!(chain_id, "EVM poller 任务失败，触发全局 shutdown: {e:#}");
                            let _ = shutdown_tx.send(true);
                        }
                    }));
                }
                // ── EVM submitter ──
                {
                    let ep = ep.clone();
                    let events_root = Arc::clone(&events_root);
                    let shutdown = shutdown_rx.clone();
                    let shutdown_tx = shutdown_tx.clone();
                    let evm_wallet = keys.evm_wallet.clone();
                    handles.push(tokio::spawn(async move {
                        let chain_id = ep.chain_id;
                        run_evm_submitter(ep, &events_root, evm_wallet, shutdown, shutdown_tx).await;
                        info!(chain_id, "EVM submitter 任务退出");
                    }));
                }
            }
            ChainKind::Svm => {
                // ── SVM Task A: sig enumerator ──
                {
                    let ep = ep.clone();
                    let sigs_dir = Arc::clone(&sigs_dir);
                    let checkpoints_dir = Arc::clone(&checkpoints_dir);
                    let shutdown = shutdown_rx.clone();
                    let shutdown_tx = shutdown_tx.clone();
                    handles.push(tokio::spawn(async move {
                        let chain_id = ep.chain_id;
                        if let Err(e) = run_svm_sig_enumerator(ep, &sigs_dir, &checkpoints_dir, shutdown).await {
                            error!(chain_id, "SVM sig enumerator 任务失败，触发全局 shutdown: {e:#}");
                            let _ = shutdown_tx.send(true);
                        }
                    }));
                }
                // ── SVM Task B: event extractor ──
                {
                    let ep = ep.clone();
                    let sigs_dir = Arc::clone(&sigs_dir);
                    let sigs_dead_dir = Arc::clone(&sigs_dead_dir);
                    let events_root = Arc::clone(&events_root);
                    let known = Arc::clone(&known);
                    let shutdown = shutdown_rx.clone();
                    let shutdown_tx = shutdown_tx.clone();
                    handles.push(tokio::spawn(async move {
                        let chain_id = ep.chain_id;
                        run_svm_event_extractor(ep, &sigs_dir, &sigs_dead_dir, &events_root, &known, shutdown, shutdown_tx).await;
                        info!(chain_id, "SVM event extractor 任务退出");
                    }));
                }
                // ── SVM Task C: submitter ──
                {
                    let ep = ep.clone();
                    let events_root = Arc::clone(&events_root);
                    let shutdown = shutdown_rx.clone();
                    let shutdown_tx = shutdown_tx.clone();
                    let svm_kp_bytes = keys.svm_keypair.to_bytes();
                    handles.push(tokio::spawn(async move {
                        let chain_id = ep.chain_id;
                        let kp = match solana_sdk::signature::Keypair::try_from(svm_kp_bytes.as_slice()) {
                            Ok(k) => k,
                            Err(e) => {
                                error!(chain_id, "重建 SVM keypair 失败，触发全局 shutdown: {e:#}");
                                let _ = shutdown_tx.send(true);
                                return;
                            }
                        };
                        run_svm_submitter(ep, &events_root, kp, shutdown).await;
                        info!(chain_id, "SVM submitter 任务退出");
                    }));
                }
            }
        }
    }

    // ── 6. 等所有 worker 退出（通常发生在收到 shutdown 之后）──────────
    futures::future::join_all(handles).await;
    info!("所有 worker 任务已退出，relayer 优雅关闭");
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════
// Poller：每条链一个，从该链拉事件 → 按 target_chain_id 分发到磁盘
// ═══════════════════════════════════════════════════════════════════════

/// EVM poller 主循环。
///
/// 不变量：
/// - 所有事件**全部落盘后**才推进 checkpoint（H1 不丢事件保证）
/// - target_chain_id 不在 known 集合的事件直接 warn 跳过（不写盘，避免无 submitter 消化导致永久积压）
/// - poll 失败 from_block 不变 → 下一轮重试同一区间
async fn run_evm_poller(
    ep: ChainEndpoint,
    events_root: &Path,
    checkpoints_dir: &Path,
    known: &HashSet<u64>,
    mut shutdown: Shutdown,
) -> Result<()> {
    let provider = Provider::<Http>::try_from(&ep.rpc_url).context("创建 EVM provider")?;
    let contract = bytes32_to_evm_address(&ep.contract)?;

    // 恢复 checkpoint；首次启动从下一个 finalized 区块开始（M7：不回扫历史）
    let mut from_block = match load_evm_checkpoint(checkpoints_dir, ep.chain_id)? {
        Some(cp) => cp.last_block,
        None => {
            let start = evm::poller::initial_from_block(&provider, ep.chain_id).await?;
            // 立即写盘，避免下次启动被 RPC 时差影响错误地从更早位置继续
            let cp = EvmCheckpoint { last_block: start };
            if let Err(e) = save_evm_checkpoint(checkpoints_dir, ep.chain_id, &cp) {
                warn!(chain_id = ep.chain_id, "保存初始 checkpoint 失败: {e:#}");
            }
            info!(
                chain_id = ep.chain_id,
                from_block = start,
                "无 checkpoint，从当前 finalized 之后开始"
            );
            start
        }
    };

    info!(
        chain_id = ep.chain_id,
        contract = ?contract,
        from_block,
        "EVM poller 启动"
    );

    loop {
        let mut catching_up = false;

        match evm::poller::poll_evm_events(
            &provider,
            contract,
            from_block,
            EVM_BLOCK_RANGE,
            ep.chain_id,
        )
        .await
        {
            Ok((events, new_from)) => {
                if events.is_empty() {
                    tracing::debug!(
                        chain_id = ep.chain_id,
                        from_block,
                        "EVM poller 本轮无新事件"
                    );
                } else {
                    info!(
                        chain_id = ep.chain_id,
                        from_block,
                        count = events.len(),
                        "EVM poller 拉取到新事件"
                    );
                }
                let mut all_persisted = true;
                for ev in &events {
                    if !known.contains(&ev.target_chain_id) {
                        warn!(
                            source_chain_id = ev.source_chain_id,
                            target_chain_id = ev.target_chain_id,
                            nonce = ev.nonce,
                            "目标链未注册，跳过该事件（不写盘）"
                        );
                        continue;
                    }
                    if let Err(e) = save_pending_event(events_root, ev) {
                        warn!(
                            chain_id = ep.chain_id,
                            nonce = ev.nonce,
                            "持久化事件失败: {e:#}"
                        );
                        all_persisted = false;
                    }
                }

                if all_persisted && new_from > from_block {
                    catching_up = evm_should_catch_up(from_block, new_from);
                    from_block = new_from;
                    let cp = EvmCheckpoint { last_block: from_block };
                    if let Err(e) = save_evm_checkpoint(checkpoints_dir, ep.chain_id, &cp) {
                        warn!(chain_id = ep.chain_id, "保存 checkpoint 失败: {e:#}");
                    }
                }
            }
            Err(e) => {
                warn!(chain_id = ep.chain_id, "EVM poll 错误: {e:#}");
            }
        }

        let interval = if catching_up { CATCHUP_DELAY } else { POLL_INTERVAL };
        if sleep_or_shutdown(interval, &mut shutdown).await {
            info!(chain_id = ep.chain_id, "EVM poller 收到 shutdown，退出");
            return Ok(());
        }
    }
}

/// SVM sig enumerator (Task A)：枚举新签名并写空文件到 sigs/{chain_id}/。
///
/// checkpoint 语义从"已处理到此 sig"变成"已枚举到此 sig"。
/// 提取/未提取的状态由 sigs/ 工作队列承载。
async fn run_svm_sig_enumerator(
    ep: ChainEndpoint,
    sigs_dir: &Path,
    checkpoints_dir: &Path,
    mut shutdown: Shutdown,
) -> Result<()> {
    let rpc = RpcClient::new_with_commitment(
        ep.rpc_url.clone(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );
    let program_id = Pubkey::new_from_array(ep.contract);

    let active_dir = sigs_dir.join(ep.chain_id.to_string());
    std::fs::create_dir_all(&active_dir)
        .with_context(|| format!("创建 sigs 子目录失败: {}", active_dir.display()))?;

    let mut last_sig = match load_svm_checkpoint(checkpoints_dir, ep.chain_id)? {
        Some(cp) => Some(Signature::from_str(&cp.last_signature).context("解析已保存的 signature")?),
        None => loop {
            match svm::poller::head_signature(&rpc, &program_id).await {
                Ok(Some(sig)) => {
                    let cp = SvmCheckpoint {
                        last_signature: sig.to_string(),
                    };
                    if let Err(e) = save_svm_checkpoint(checkpoints_dir, ep.chain_id, &cp) {
                        warn!(chain_id = ep.chain_id, "保存初始 checkpoint 失败: {e:#}");
                    }
                    info!(
                        chain_id = ep.chain_id,
                        head = %sig,
                        "无 checkpoint，从当前 head 之后开始"
                    );
                    break Some(sig);
                }
                Ok(None) => {
                    info!(
                        chain_id = ep.chain_id,
                        "桥程序尚无任何 tx，等待第一笔 tx 出现再做锚点（避免回扫历史）"
                    );
                }
                Err(e) => {
                    warn!(chain_id = ep.chain_id, "拉取 head signature 失败，稍后重试: {e:#}");
                }
            }
            if sleep_or_shutdown(POLL_INTERVAL, &mut shutdown).await {
                info!(chain_id = ep.chain_id, "SVM sig enumerator 启动期收到 shutdown，退出");
                return Ok(());
            }
        },
    };

    info!(
        chain_id = ep.chain_id,
        program_id = %program_id,
        "SVM sig enumerator 启动"
    );

    loop {
        // 每轮都一路翻页到 checkpoint、拿到全部新签名（不截断，见
        // enumerate_new_signatures 文档）。若本轮签名数达到 SVM_MAX_SIGS，说明刚
        // 清了一大批积压，切到 CATCHUP_DELAY 立刻再拉一轮以尽快追平链头。
        let mut catching_up = false;

        match svm::poller::enumerate_new_signatures(
            &rpc,
            &program_id,
            last_sig.as_ref(),
            SVM_SIG_BATCH,
        )
        .await
        {
            Ok(new_sigs) => {
                if !new_sigs.is_empty() {
                    let mut last_persisted: Option<Signature> = None;
                    for sig in &new_sigs {
                        if let Err(e) = svm::sig_queue::save_new_sig(&active_dir, sig) {
                            warn!(chain_id = ep.chain_id, sig = %sig, "写 sig 文件失败: {e:#}");
                            break;
                        }
                        last_persisted = Some(*sig);
                    }
                    if let Some(newest) = last_persisted {
                        last_sig = Some(newest);
                        let cp = SvmCheckpoint {
                            last_signature: newest.to_string(),
                        };
                        if let Err(e) = save_svm_checkpoint(checkpoints_dir, ep.chain_id, &cp) {
                            warn!(chain_id = ep.chain_id, "保存 checkpoint 失败: {e:#}");
                        }
                    }
                    catching_up = svm_enumerator_should_catch_up(new_sigs.len());
                    info!(
                        chain_id = ep.chain_id,
                        total = new_sigs.len(),
                        persisted = last_persisted.map(|s| s.to_string()).unwrap_or_default(),
                        catching_up,
                        "已枚举新 SVM 签名并写入 sigs 队列"
                    );
                }
            }
            Err(e) => {
                warn!(chain_id = ep.chain_id, "SVM sig 枚举错误: {e:#}");
            }
        }

        let interval = if catching_up { CATCHUP_DELAY } else { POLL_INTERVAL };
        if sleep_or_shutdown(interval, &mut shutdown).await {
            info!(chain_id = ep.chain_id, "SVM sig enumerator 收到 shutdown，退出");
            return Ok(());
        }
    }
}

/// SVM event extractor (Task B)：读 sigs/{chain_id}/ → 拉 getTransaction → 落事件。
///
/// 内部持有 `states: HashMap<Signature, AttemptState>` 用于重试计数与节流。
/// 进程重启后为空，磁盘上残留的 active sig 按 fresh（attempt_count=0）重新处理。
async fn run_svm_event_extractor(
    ep: ChainEndpoint,
    sigs_dir: &Path,
    sigs_dead_dir: &Path,
    events_root: &Path,
    known: &HashSet<u64>,
    mut shutdown: Shutdown,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
) {
    let rpc = RpcClient::new_with_commitment(
        ep.rpc_url.clone(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );

    let active_dir = sigs_dir.join(ep.chain_id.to_string());
    let dead_dir = sigs_dead_dir.join(ep.chain_id.to_string());
    if let Err(e) = std::fs::create_dir_all(&active_dir) {
        error!(chain_id = ep.chain_id, "创建 sigs 子目录失败，触发全局 shutdown: {e:#}");
        let _ = shutdown_tx.send(true);
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&dead_dir) {
        error!(chain_id = ep.chain_id, "创建 sigs_dead 子目录失败，触发全局 shutdown: {e:#}");
        let _ = shutdown_tx.send(true);
        return;
    }

    let mut states: HashMap<Signature, svm::sig_queue::AttemptState> = HashMap::new();
    let mut round_count: u64 = 0;

    info!(
        chain_id = ep.chain_id,
        "SVM event extractor 启动"
    );

    loop {
        if sleep_or_shutdown(jittered_submit_interval(), &mut shutdown).await {
            info!(chain_id = ep.chain_id, "SVM event extractor 收到 shutdown，退出");
            return;
        }

        let active_sigs = match svm::sig_queue::list_active_sigs(&active_dir) {
            Ok(v) => v,
            Err(e) => {
                warn!(chain_id = ep.chain_id, "列出 active sigs 失败: {e:#}");
                continue;
            }
        };

        if active_sigs.is_empty() {
            continue;
        }

        round_count = round_count.wrapping_add(1);
        if round_count % 100 == 0 {
            let active_set: HashSet<Signature> = active_sigs.iter().copied().collect();
            states.retain(|sig, _| active_set.contains(sig));
        }

        let now = now_unix();

        for sig in active_sigs {
            if is_shutdown_requested(&shutdown) {
                return;
            }

            let state = states.entry(sig).or_default();

            // 重试节流
            if now.saturating_sub(state.last_attempt_at) < SVM_EXTRACT_MIN_RETRY_INTERVAL_SECS {
                continue;
            }

            match svm::poller::fetch_and_extract_events(&rpc, &sig).await {
                Ok(events) => {
                    let mut all_saved = true;
                    for ev in &events {
                        if !known.contains(&ev.target_chain_id) {
                            warn!(
                                chain_id = ep.chain_id,
                                source_chain_id = ev.source_chain_id,
                                target_chain_id = ev.target_chain_id,
                                nonce = ev.nonce,
                                "目标链未注册，跳过该事件（不写盘）"
                            );
                            continue;
                        }
                        if let Err(e) = save_pending_event(events_root, ev) {
                            warn!(
                                chain_id = ep.chain_id,
                                nonce = ev.nonce,
                                "持久化事件失败: {e:#}"
                            );
                            all_saved = false;
                        }
                    }
                    if all_saved {
                        if let Err(e) = svm::sig_queue::delete_sig(&active_dir, &sig) {
                            warn!(chain_id = ep.chain_id, sig = %sig, "删除已提取 sig 文件失败: {e:#}");
                        }
                        states.remove(&sig);
                    } else {
                        warn!(
                            chain_id = ep.chain_id,
                            sig = %sig,
                            "部分事件落盘失败，保留 sig 文件下轮重试"
                        );
                    }
                }
                Err(e) => {
                    state.attempt_count += 1;
                    state.last_attempt_at = now;

                    if state.attempt_count >= SVM_EXTRACT_MAX_ATTEMPTS {
                        match svm::sig_queue::move_to_dead_letter(&active_dir, &dead_dir, &sig) {
                            Ok(()) => {
                                error!(
                                    chain_id = ep.chain_id,
                                    sig = %sig,
                                    attempts = state.attempt_count,
                                    "SVM sig 提取连续失败已转入 DLQ，需人工核查: {e:#}"
                                );
                                states.remove(&sig);
                            }
                            Err(mv_err) => {
                                error!(
                                    chain_id = ep.chain_id,
                                    sig = %sig,
                                    "移动 sig 到 DLQ 失败: {mv_err}"
                                );
                            }
                        }
                    } else {
                        warn!(
                            chain_id = ep.chain_id,
                            sig = %sig,
                            attempt = state.attempt_count,
                            max = SVM_EXTRACT_MAX_ATTEMPTS,
                            "SVM sig 提取失败，稍后重试: {e:#}"
                        );
                    }
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Submitter：每条链一个，扫 events/{chain_id}/ 串行处理
// ═══════════════════════════════════════════════════════════════════════

/// EVM submitter 主循环（pipelined submit + async confirmation）。
///
/// - 每轮 1-2s jitter 扫自己的 events 目录
/// - **不再串行等 N confs**：广播即写盘 → 立刻处理下一笔；成熟度由后续轮次检查
/// - 单事件每轮耗时 ~RPC 延迟（200ms 量级），不再被 12 块（~2.4min）阻塞
/// - 上层 `process_event_for_evm` 是状态机，根据 entry.submission 决定行动
async fn run_evm_submitter(
    ep: ChainEndpoint,
    events_root: &Path,
    wallet: LocalWallet,
    mut shutdown: Shutdown,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
) {
    let provider = match Provider::<Http>::try_from(&ep.rpc_url) {
        Ok(p) => p,
        Err(e) => {
            error!(chain_id = ep.chain_id, "EVM submitter 创建 provider 失败，触发全局 shutdown: {e:#}");
            let _ = shutdown_tx.send(true);
            return;
        }
    };
    let contract = match bytes32_to_evm_address(&ep.contract) {
        Ok(a) => a,
        Err(e) => {
            error!(chain_id = ep.chain_id, "EVM submitter 解析合约地址失败，触发全局 shutdown: {e:#}");
            let _ = shutdown_tx.send(true);
            return;
        }
    };

    // SignerMiddleware 只构建一次，整个 submitter 生命周期复用。
    // 内部不缓存 nonce（每次 send_transaction 都 eth_getTransactionCount(pending)），
    // 所以多笔事件复用同一个 client 不会冲突；也省掉了每事件 wallet.clone() + new(...) 的开销。
    let client: EvmClient =
        SignerMiddleware::new(provider.clone(), wallet.with_chain_id(ep.chain_id));

    info!(
        chain_id = ep.chain_id,
        contract = ?contract,
        "EVM submitter 启动"
    );

    loop {
        let mut pending = match load_all_pending_events(events_root, ep.chain_id) {
            Ok(v) => v,
            Err(e) => {
                warn!(chain_id = ep.chain_id, "扫描事件目录失败: {e:#}");
                Vec::new()
            }
        };

        // 没有待处理事件就直接 sleep，省一次 latest 查询
        if pending.is_empty() {
            if sleep_or_shutdown(jittered_submit_interval(), &mut shutdown).await {
                return;
            }
            continue;
        }

        // 每轮拉一次 latest，本轮所有 entry 复用：
        // - check_nonce_status (EVM) 按 NonceCheckBlock 决定用 latest 或 safe_head
        // - check_tx_maturity 用 latest 算 depth
        // 即使该 latest 在本轮处理过程中已经被新块超过，也只会让结论更保守，不会误判。
        let latest = match provider.get_block_number().await {
            Ok(n) => n.as_u64(),
            Err(e) => {
                warn!(chain_id = ep.chain_id, "查询 latest block 失败，本轮跳过: {e:#}");
                if sleep_or_shutdown(jittered_submit_interval(), &mut shutdown).await {
                    return;
                }
                continue;
            }
        };

        // 打乱顺序：多 relayer 实例不会都从最早的 nonce 开始撞同一笔事件，
        // 把"被某 relayer 抢先上链"的概率均匀分布到所有 pending 事件上，
        // 显著减少阈值投票场景下的 revert 浪费。
        pending.shuffle(&mut rand::thread_rng());

        for entry in pending {
            if is_shutdown_requested(&shutdown) {
                info!(
                    chain_id = ep.chain_id,
                    "EVM submitter 收到 shutdown，跳过剩余事件"
                );
                return;
            }
            process_evm_entry(
                events_root,
                &client,
                &provider,
                contract,
                ep.chain_id,
                &ep.contract,
                entry,
                latest,
            )
            .await;
        }

        if sleep_or_shutdown(jittered_submit_interval(), &mut shutdown).await {
            return;
        }
    }
}

/// SVM submitter 主循环（pipelined submit + async confirmation）。
///
/// 与改造前的差异：
/// - 不再用 `send_and_confirm_transaction` 阻塞等 finalized（~13s/事件），
///   改成广播即写盘 → 立刻处理下一笔；finalized 由后续轮次的 `check_tx_maturity` 检测
/// - RPC 全部走 `nonblocking::RpcClient`，不会再阻塞 tokio runtime 线程（H1）
///
/// 与 EVM submitter 共享的差异：
/// - 启动时 ep.svm 可能为 None（peer SVM 启动期 RPC 不通），每轮先 lazy fetch
///   SvmConfig；拿不到就跳过本轮提交，下一轮再试
/// - 不需要每轮拉 latest_block：SVM `confirmation_status` 字段直接给 finalized 状态，
///   不像 EVM 要按 block depth 算 N confs
async fn run_svm_submitter(
    ep: ChainEndpoint,
    events_root: &Path,
    keypair: solana_sdk::signature::Keypair,
    mut shutdown: Shutdown,
) {
    let rpc = RpcClient::new_with_commitment(
        ep.rpc_url.clone(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );
    let program_id = Pubkey::new_from_array(ep.contract);

    let mut svm_cfg: Option<SvmConfig> = ep.svm.clone();
    // M1：lazy fetch 连续失败计数。成功后清零；每达到 SVM_LAZY_FETCH_ERROR_EVERY
    // 次失败升级一次 error!，方便接监控告警，避免持续 warn 被淹没。
    let mut consecutive_fetch_fails: u32 = 0;

    info!(
        chain_id = ep.chain_id,
        program_id = %program_id,
        has_svm_config = svm_cfg.is_some(),
        "SVM submitter 启动"
    );

    loop {
        // ── 步骤 0: 确保拿到 SvmConfig（1024 必然已有；某些 peer 可能启动时未取到）──
        if svm_cfg.is_none() {
            // 从注册表查程序形态（Hub for 1024，Leaf for Solana 等）。
            // 注册表缺失说明运维加了新 SVM 链却没在 chain_registry 配 svm_program_kind ——
            // 升级 error! 提示，本轮跳过提交，等运维修复后下轮恢复。
            let Some(kind) = chain_registry::svm_program_kind(ep.chain_id) else {
                error!(
                    chain_id = ep.chain_id,
                    "SVM 链未在 chain_registry 配置 svm_program_kind，无法构造 confirm_event；\
                     请先在 chain_registry.rs 注册"
                );
                if sleep_or_shutdown(jittered_submit_interval(), &mut shutdown).await {
                    return;
                }
                continue;
            };
            match fetch_svm_config(&rpc, &program_id, kind).await {
                Ok(cfg) => {
                    if consecutive_fetch_fails > 0 {
                        info!(
                            chain_id = ep.chain_id,
                            recovered_after_fails = consecutive_fetch_fails,
                            "SVM lazy fetch 已从持续失败状态恢复"
                        );
                    }
                    consecutive_fetch_fails = 0;
                    info!(
                        chain_id = ep.chain_id,
                        usdc_mint = %cfg.usdc_mint,
                        token_program = %cfg.token_program,
                        program_kind = %cfg.program_kind,
                        "SVM submitter lazy 发现链上配置成功"
                    );
                    svm_cfg = Some(cfg);
                }
                Err(e) => {
                    consecutive_fetch_fails = consecutive_fetch_fails.saturating_add(1);
                    // 每 SVM_LAZY_FETCH_ERROR_EVERY 次升级 error!；其余 warn!。
                    // 这样 1 分钟左右（30 × ~1.5s）持续失败会触发一次 error。
                    if consecutive_fetch_fails % SVM_LAZY_FETCH_ERROR_EVERY == 0 {
                        error!(
                            chain_id = ep.chain_id,
                            program_kind = %kind,
                            consecutive_fails = consecutive_fetch_fails,
                            "SVM peer 持续无法获取 BridgeState（请检查 program_id / RPC 可达性）: {e:#}"
                        );
                    } else {
                        warn!(
                            chain_id = ep.chain_id,
                            program_kind = %kind,
                            consecutive_fails = consecutive_fetch_fails,
                            "尚未取到 SVM BridgeState，本轮跳过提交: {e:#}"
                        );
                    }
                    if sleep_or_shutdown(jittered_submit_interval(), &mut shutdown).await {
                        return;
                    }
                    continue;
                }
            }
        }
        let cfg = svm_cfg.as_ref().expect("已确保 Some");

        // ── 步骤 1: 串行处理 events/{chain_id}/ ──
        let mut pending = match load_all_pending_events(events_root, ep.chain_id) {
            Ok(v) => v,
            Err(e) => {
                warn!(chain_id = ep.chain_id, "扫描事件目录失败: {e:#}");
                Vec::new()
            }
        };

        // 没有待处理事件就直接 sleep
        if pending.is_empty() {
            if sleep_or_shutdown(jittered_submit_interval(), &mut shutdown).await {
                return;
            }
            continue;
        }

        // 打乱顺序：多 relayer 实例不会都从最早的 nonce 开始撞同一笔事件，
        // 把"被某 relayer 抢先上链"的概率均匀分布到所有 pending 事件上，
        // 显著减少阈值投票场景下的 revert 浪费。
        pending.shuffle(&mut rand::thread_rng());

        for entry in pending {
            if is_shutdown_requested(&shutdown) {
                info!(
                    chain_id = ep.chain_id,
                    "SVM submitter 收到 shutdown，跳过剩余事件"
                );
                return;
            }
            process_svm_entry(
                events_root,
                &rpc,
                &program_id,
                cfg.program_kind,
                &cfg.usdc_mint,
                &cfg.token_program,
                &keypair,
                ep.chain_id,
                &ep.contract,
                entry,
            )
            .await;
        }

        if sleep_or_shutdown(jittered_submit_interval(), &mut shutdown).await {
            return;
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 单条事件的提交逻辑
// ═══════════════════════════════════════════════════════════════════════

/// SVM submitter 单条 entry 的状态机推进（pipelined）。
///
/// 镜像 `process_evm_entry` 的设计：
/// 1. **No submission** → 链上已处理则删文件；否则广播 tx 并写盘 submission（不等 finalized）
/// 2. **Has submission** → 查 maturity：
///    - `Confirmed`：再 verify 一次 nonce 是否真的被处理 → 删文件；否则视为异常清掉重试
///    - `Pending`：什么都不做（下轮再查）
///    - `Reverted`：清 submission（多见于 AlreadyProcessed），下轮 check_nonce 再判
///    - `NotYetLanded`：超过 STALE_PENDING_SVM_TX_SECS 视为 dropped → 清 submission 重广播
///
/// 全程任何一步只调 1-2 次 RPC，绝不阻塞等 finalized commitment。
#[allow(clippy::too_many_arguments)]
async fn process_svm_entry(
    events_root: &Path,
    rpc: &RpcClient,
    program_id: &Pubkey,
    program_kind: SvmProgramKind,
    usdc_mint: &Pubkey,
    token_program: &Pubkey,
    keypair: &solana_sdk::signature::Keypair,
    chain_id: u64,
    expected_contract: &[u8; 32],
    mut entry: PendingEntry,
) {
    let event = entry.event.clone();
    let source_chain_id = event.source_chain_id;
    let nonce = event.nonce;

    if event.target_chain_id != chain_id || &event.target_contract != expected_contract {
        warn!(
            chain_id, source_chain_id, nonce,
            event_target_chain = event.target_chain_id,
            "事件 target_chain_id/target_contract 与本 submitter 不匹配，删文件"
        );
        if let Err(e) = delete_pending_event(events_root, &event) {
            warn!(chain_id, source_chain_id, nonce, "删除不匹配事件文件失败: {e:#}");
        }
        return;
    }

    // ── 分支 A：尚未广播 —— 两步检查（confirmed → finalized）──
    let Some(sub) = entry.submission.clone() else {
        // step1: 用 confirmed 快速感知
        match svm::submitter::check_nonce_status(
            rpc, program_id, program_kind, source_chain_id, nonce, &keypair.pubkey(),
            CommitmentConfig::confirmed(),
        ).await {
            Ok(svm::submitter::NonceStatus::FullyProcessed | svm::submitter::NonceStatus::AlreadyConfirmedByUs) => {
                // step2: 用 finalized 确认可安全删文件
                match svm::submitter::check_nonce_status(
                    rpc, program_id, program_kind, source_chain_id, nonce, &keypair.pubkey(),
                    CommitmentConfig::finalized(),
                ).await {
                    Ok(svm::submitter::NonceStatus::FullyProcessed | svm::submitter::NonceStatus::AlreadyConfirmedByUs) => {
                        info!(chain_id, source_chain_id, nonce, "Nonce 在 SVM finalized 已确认，删文件");
                        if let Err(e) = delete_pending_event(events_root, &event) {
                            warn!(chain_id, source_chain_id, nonce, "删除已处理事件文件失败: {e:#}");
                        }
                    }
                    Ok(svm::submitter::NonceStatus::PendingOurVote) => {
                        tracing::debug!(
                            chain_id, source_chain_id, nonce,
                            "confirmed 已处理但 finalized 未确认，等下一轮（不提交）"
                        );
                    }
                    Err(e) => {
                        warn!(chain_id, source_chain_id, nonce, "step2 查询 SVM nonce 状态失败: {e:#}");
                    }
                }
                return;
            }
            Ok(svm::submitter::NonceStatus::PendingOurVote) => {} // 真正需要广播
            Err(e) => {
                warn!(chain_id, source_chain_id, nonce, "查询 SVM nonce 状态失败: {e:#}");
                return;
            }
        }
        // 广播（不等 finalized）
        match svm::submitter::broadcast_confirm_event(
            rpc, program_id, program_kind, keypair, usdc_mint, token_program, &event,
        )
        .await
        {
            Ok(sig) => {
                entry.submission = Some(Submission {
                    tx_hash: sig.to_string(),
                    sent_at_unix: now_unix(),
                    mined_block: None,
                });
                if let Err(e) = update_pending_entry(events_root, &entry) {
                    error!(
                        chain_id,
                        source_chain_id,
                        nonce,
                        tx = %sig,
                        "广播成功但写入 submission 失败：进程重启后会重广播: {e:#}"
                    );
                }
            }
            Err(e) => {
                warn!(chain_id, source_chain_id, nonce, "广播 SVM confirm_event 失败: {e:#}");
            }
        }
        return;
    };

    // ── 分支 B：已广播，查成熟度 ──
    let sig = match Signature::from_str(&sub.tx_hash) {
        Ok(s) => s,
        Err(e) => {
            warn!(
                chain_id, source_chain_id, nonce,
                "submission.tx_hash 格式异常 ({e})，清掉 submission 下轮重广播"
            );
            entry.submission = None;
            if let Err(e) = update_pending_entry(events_root, &entry) {
                warn!(chain_id, source_chain_id, nonce, "清 submission 写盘失败: {e:#}");
            }
            return;
        }
    };

    match svm::submitter::check_tx_maturity(rpc, sig).await {
        Ok(svm::submitter::TxMaturity::Confirmed { slot }) => {
            match svm::submitter::check_nonce_status(
                rpc, program_id, program_kind, source_chain_id, nonce, &keypair.pubkey(),
                CommitmentConfig::finalized(),
            ).await {
                Ok(svm::submitter::NonceStatus::FullyProcessed) => {
                    info!(
                        chain_id, source_chain_id, nonce,
                        tx = %sub.tx_hash, slot,
                        "SVM confirm_event 已 finalized 且 nonce 已处理，删文件"
                    );
                    if let Err(e) = delete_pending_event(events_root, &event) {
                        warn!(chain_id, source_chain_id, nonce, "删除已确认事件文件失败: {e:#}");
                    }
                }
                Ok(svm::submitter::NonceStatus::AlreadyConfirmedByUs) => {
                    info!(
                        chain_id, source_chain_id, nonce,
                        tx = %sub.tx_hash, slot,
                        "SVM confirm_event 已 finalized，本 relayer 投票已记录，删文件"
                    );
                    if let Err(e) = delete_pending_event(events_root, &event) {
                        warn!(chain_id, source_chain_id, nonce, "删除已投票事件文件失败: {e:#}");
                    }
                }
                Ok(svm::submitter::NonceStatus::PendingOurVote) => {
                    warn!(
                        chain_id, source_chain_id, nonce,
                        tx = %sub.tx_hash,
                        "tx 报告 finalized 但链上未见本 relayer 投票，清 submission 重判"
                    );
                    entry.submission = None;
                    if let Err(e) = update_pending_entry(events_root, &entry) {
                        warn!(chain_id, source_chain_id, nonce,
                            "异常状态清 submission 写盘失败: {e:#}");
                    }
                }
                Err(e) => {
                    warn!(chain_id, source_chain_id, nonce,
                        "verify 阶段查询 SVM nonce 状态失败: {e:#}");
                }
            }
        }
        Ok(svm::submitter::TxMaturity::Pending { slot }) => {
            tracing::debug!(
                chain_id,
                source_chain_id,
                nonce,
                tx = %sub.tx_hash,
                slot,
                "SVM tx 已 land，等 finalized commitment"
            );
        }
        Ok(svm::submitter::TxMaturity::Reverted { slot }) => {
            warn!(
                chain_id, source_chain_id, nonce,
                tx = %sub.tx_hash, slot,
                "SVM tx revert，清 submission 下一轮由 Branch A 两步检查处理"
            );
            entry.submission = None;
            if let Err(e) = update_pending_entry(events_root, &entry) {
                warn!(chain_id, source_chain_id, nonce, "revert 后清 submission 写盘失败: {e:#}");
            }
        }
        Ok(svm::submitter::TxMaturity::NotYetLanded) => {
            let age = now_unix().saturating_sub(sub.sent_at_unix);
            if age > STALE_PENDING_SVM_TX_SECS {
                // SVM 不像 EVM 有 nonce gap 问题：每笔 tx 用 fresh blockhash 独立签名，
                // 直接清 submission 下轮重广播即可，没有"老 tx 卡住新 tx"的现象。
                warn!(
                    chain_id,
                    source_chain_id,
                    nonce,
                    tx = %sub.tx_hash,
                    age_s = age,
                    "SVM tx 长时间未 land（blockhash 多半已过期），清 submission 下轮重广播"
                );
                entry.submission = None;
                if let Err(e) = update_pending_entry(events_root, &entry) {
                    warn!(chain_id, source_chain_id, nonce, "stale 后清 submission 写盘失败: {e:#}");
                }
            } else {
                tracing::debug!(
                    chain_id,
                    source_chain_id,
                    nonce,
                    age_s = age,
                    "SVM tx 等 land 中（blockhash 仍在有效期）"
                );
            }
        }
        Err(e) => {
            warn!(chain_id, source_chain_id, nonce, "查询 SVM tx 成熟度失败: {e:#}");
        }
    }
}

/// EVM submitter 单条 entry 的状态机推进。
///
/// 状态分支：
/// 1. **Branch A（无 submission）** → 两步检查（Latest → SafeHead）：
///    Latest 已处理则等 SafeHead 确认删文件；否则 EIP-1559 广播并写盘 submission
/// 2. **Branch B（有 submission）** → 查 maturity：
///    - `Confirmed`：SafeHead check_nonce_status 兜底校验 → 删文件或清 submission
///    - `Pending`：缓存 mined_block，下轮走 fast-path 免拉 receipt
///    - `Reverted`：直接清 submission，下轮 Branch A 重判
///    - `NotYetMined`：非 stale 等待；stale 路径 get_pending → evict/清 submission，
///      mempool 中则 self-transfer 推进 nonce 再清 submission
///
/// 全程任何一步只调 1-2 次 RPC，绝不阻塞等 N 个 block。
#[allow(clippy::too_many_arguments)]
async fn process_evm_entry(
    events_root: &Path,
    client: &EvmClient,
    provider: &Provider<Http>,
    contract: Address,
    chain_id: u64,
    expected_contract: &[u8; 32],
    mut entry: PendingEntry,
    latest_block: u64,
) {
    let event = entry.event.clone();
    let source_chain_id = event.source_chain_id;
    let nonce = event.nonce;

    if event.target_chain_id != chain_id || &event.target_contract != expected_contract {
        warn!(
            chain_id, source_chain_id, nonce,
            event_target_chain = event.target_chain_id,
            "事件 target_chain_id/target_contract 与本 submitter 不匹配，删文件"
        );
        if let Err(e) = delete_pending_event(events_root, &event) {
            warn!(chain_id, source_chain_id, nonce, "删除不匹配事件文件失败: {e:#}");
        }
        return;
    }

    let relayer_addr = client.signer().address();

    // ── 分支 A：尚未广播 —— 两步检查（Latest → SafeHead）──
    let Some(sub) = entry.submission.clone() else {
        // step1: 用 Latest 快速感知是否已有人处理
        match evm::submitter::check_nonce_status(
            provider, contract, chain_id, nonce, relayer_addr, latest_block,
            evm::submitter::NonceCheckBlock::Latest,
        ).await {
            Ok(evm::submitter::NonceStatus::FullyProcessed | evm::submitter::NonceStatus::AlreadyConfirmedByUs) => {
                // step2: 用 SafeHead 确认 confs 已满足，安全删文件
                match evm::submitter::check_nonce_status(
                    provider, contract, chain_id, nonce, relayer_addr, latest_block,
                    evm::submitter::NonceCheckBlock::SafeHead,
                ).await {
                    Ok(evm::submitter::NonceStatus::FullyProcessed | evm::submitter::NonceStatus::AlreadyConfirmedByUs) => {
                        info!(chain_id, source_chain_id, nonce, "Nonce 在 EVM SafeHead 已确认，删文件");
                        if let Err(e) = delete_pending_event(events_root, &event) {
                            warn!(chain_id, source_chain_id, nonce, "删除已处理事件文件失败: {e:#}");
                        }
                    }
                    Ok(evm::submitter::NonceStatus::PendingOurVote) => {
                        tracing::debug!(
                            chain_id, source_chain_id, nonce,
                            "Latest 已处理但 SafeHead 未确认，等下一轮（不提交）"
                        );
                    }
                    Err(e) => {
                        warn!(chain_id, source_chain_id, nonce, "step2 查询 EVM nonce 状态失败: {e:#}");
                    }
                }
                return;
            }
            Ok(evm::submitter::NonceStatus::PendingOurVote) => {} // 真正需要广播
            Err(e) => {
                warn!(chain_id, source_chain_id, nonce, "查询 EVM nonce 状态失败: {e:#}");
                return;
            }
        }
        // 广播 EIP-1559 tx（不等回执）
        match evm::submitter::broadcast_confirm_event(client, contract, chain_id, &event).await {
            Ok(tx_hash) => {
                entry.submission = Some(Submission {
                    tx_hash: format!("{tx_hash:?}"),
                    sent_at_unix: now_unix(),
                    mined_block: None,
                });
                if let Err(e) = update_pending_entry(events_root, &entry) {
                    error!(
                        chain_id,
                        source_chain_id,
                        nonce,
                        tx_hash = ?tx_hash,
                        "广播成功但写入 submission 失败：进程重启后会重广播浪费 gas: {e:#}"
                    );
                }
            }
            Err(e) => {
                warn!(chain_id, source_chain_id, nonce, "广播 EVM confirmEvent 失败: {e:#}");
            }
        }
        return;
    };

    // ── 分支 B：已广播，查成熟度 ──
    let tx_hash = match parse_tx_hash(&sub.tx_hash) {
        Ok(h) => h,
        Err(e) => {
            warn!(
                chain_id, source_chain_id, nonce,
                "submission.tx_hash 格式异常 ({e})，清掉 submission 下轮重广播"
            );
            entry.submission = None;
            if let Err(e) = update_pending_entry(events_root, &entry) {
                warn!(
                    chain_id, source_chain_id, nonce,
                    "清 submission 写盘失败（下轮仍按原 submission 处理）: {e:#}"
                );
            }
            return;
        }
    };

    match evm::submitter::check_tx_maturity(
        provider,
        chain_id,
        tx_hash,
        latest_block,
        sub.mined_block,
    )
    .await
    {
        Ok(evm::submitter::TxMaturity::Confirmed { mined_block }) => {
            // N confs 已满足；再 verify 一次链上状态（防 reorg 边界 case）
            match evm::submitter::check_nonce_status(
                provider, contract, chain_id, nonce, relayer_addr, latest_block,
                evm::submitter::NonceCheckBlock::SafeHead,
            ).await {
                Ok(evm::submitter::NonceStatus::FullyProcessed) => {
                    info!(
                        chain_id, source_chain_id, nonce,
                        tx_hash = %sub.tx_hash, mined_block,
                        "EVM confirmEvent 已最终确认，删文件"
                    );
                    if let Err(e) = delete_pending_event(events_root, &event) {
                        warn!(chain_id, source_chain_id, nonce, "删除已确认事件文件失败: {e:#}");
                    }
                }
                Ok(evm::submitter::NonceStatus::AlreadyConfirmedByUs) => {
                    info!(
                        chain_id, source_chain_id, nonce,
                        tx_hash = %sub.tx_hash, mined_block,
                        "tx finalized 且本 relayer 已投票（nonce 尚未达阈值），删文件"
                    );
                    if let Err(e) = delete_pending_event(events_root, &event) {
                        warn!(chain_id, source_chain_id, nonce, "删除已投票事件文件失败: {e:#}");
                    }
                }
                Ok(evm::submitter::NonceStatus::PendingOurVote) => {
                    warn!(
                        chain_id, source_chain_id, nonce,
                        tx_hash = %sub.tx_hash,
                        "tx finalized 但合约 nonce 未处理且本 relayer 未确认（疑似 reorg），清 submission 重试"
                    );
                    entry.submission = None;
                    if let Err(e) = update_pending_entry(events_root, &entry) {
                        warn!(chain_id, source_chain_id, nonce, "reorg 后清 submission 写盘失败: {e:#}");
                    }
                }
                Err(e) => {
                    warn!(chain_id, source_chain_id, nonce, "verify 阶段查询 nonce 状态失败: {e:#}");
                }
            }
        }
        Ok(evm::submitter::TxMaturity::Pending { mined_block, current_depth }) => {
            // 缓存 mined_block：下轮 check_tx_maturity 走 fast-path，跳过 receipt 调用。
            // 只在变化时写盘，避免重复 IO。
            let was_cached = sub.mined_block.is_some();
            if sub.mined_block != Some(mined_block) {
                entry.submission = Some(Submission { mined_block: Some(mined_block), ..sub });
                if let Err(e) = update_pending_entry(events_root, &entry) {
                    warn!(
                        chain_id, source_chain_id, nonce,
                        "缓存 mined_block 写盘失败（下轮会重新拉一次 receipt）: {e:#}"
                    );
                }
            }
            if !was_cached {
                info!(
                    chain_id,
                    source_chain_id,
                    nonce,
                    mined_block,
                    current_depth,
                    "EVM confirmEvent 已 mined，等待 confirmations 成熟"
                );
            }
        }
        Ok(evm::submitter::TxMaturity::Reverted { mined_block, gas_used }) => {
            warn!(
                chain_id, source_chain_id, nonce,
                tx_hash = %sub.tx_hash, mined_block, gas_used,
                "EVM confirmEvent revert，清 submission 下一轮由 Branch A 两步检查处理"
            );
            entry.submission = None;
            if let Err(e) = update_pending_entry(events_root, &entry) {
                warn!(chain_id, source_chain_id, nonce, "revert 后清 submission 写盘失败: {e:#}");
            }
        }
        Ok(evm::submitter::TxMaturity::NotYetMined) => {
            let age = now_unix().saturating_sub(sub.sent_at_unix);
            let stale_secs = chain_registry::stale_pending_tx_secs(chain_id)
                .unwrap_or(STALE_PENDING_TX_SECS_FALLBACK);
            if age <= stale_secs {
                tracing::debug!(
                    chain_id, source_chain_id, nonce, age_s = age,
                    "tx 仍在 mempool 等 mined"
                );
            } else {
                // stale: get_pending → self-transfer 或清 submission
                match evm::submitter::get_pending_transaction(provider, tx_hash).await {
                    Err(e) => {
                        warn!(chain_id, source_chain_id, nonce, age_s = age,
                            "stale tx 查询 mempool 失败，下轮再判: {e:#}");
                    }
                    Ok(None) => {
                        warn!(chain_id, source_chain_id, nonce, age_s = age,
                            "stale tx 已被 evict，清 submission 下轮 Branch A 重广播");
                        entry.submission = None;
                        if let Err(e) = update_pending_entry(events_root, &entry) {
                            warn!(chain_id, source_chain_id, nonce,
                                "清 submission 写盘失败: {e:#}");
                        }
                    }
                    Ok(Some(old_tx)) => {
                        if old_tx.block_number.is_some() {
                            tracing::debug!(chain_id, source_chain_id, nonce,
                                "tx 刚上链，跳过 self-transfer，下轮走成熟度检查");
                        } else {
                            match evm::submitter::send_self_transfer_to_unblock(
                                client, chain_id, &old_tx,
                            ).await {
                                Ok(_self_hash) => {
                                    warn!(chain_id, source_chain_id, nonce, age_s = age,
                                        "self-transfer 已广播，清 submission 下轮 Branch A 处理");
                                    entry.submission = None;
                                    if let Err(e) = update_pending_entry(events_root, &entry) {
                                        warn!(chain_id, source_chain_id, nonce,
                                            "清 submission 写盘失败: {e:#}");
                                    }
                                }
                                Err(e) => {
                                    error!(chain_id, source_chain_id, nonce, age_s = age,
                                        "self-transfer 失败，需人工检查账户余额: {e:#}");
                                }
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            warn!(chain_id, source_chain_id, nonce, "查询 tx 成熟度失败: {e:#}");
        }
    }
}

/// 解析 0x-前缀 hex 形式的 tx hash 字符串为 ethers `TxHash` (`H256`)。
fn parse_tx_hash(s: &str) -> Result<ethers::types::TxHash> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(stripped).context("tx_hash 不是合法 hex")?;
    if bytes.len() != 32 {
        bail!("tx_hash 长度应为 32 字节，实际 {} 字节", bytes.len());
    }
    Ok(ethers::types::TxHash::from_slice(&bytes))
}

// ═══════════════════════════════════════════════════════════════════════
// 工具
// ═══════════════════════════════════════════════════════════════════════

/// 把 32 字节 contract 转成 EVM Address（取后 20B）。
///
/// M4：前 12 字节必须是零填充，否则说明配置错（管理员误把 SVM Pubkey 当
/// EVM 地址写入），提前 fail-fast 比静默截断更安全。
fn bytes32_to_evm_address(bytes32: &[u8; 32]) -> Result<Address> {
    if !bytes32[..12].iter().all(|b| *b == 0) {
        bail!(
            "contract 前 12 字节非零，无法解释为 EVM 地址: 0x{}",
            hex::encode(bytes32)
        );
    }
    Ok(Address::from_slice(&bytes32[12..]))
}

/// 计算 `isRelayer(address)` 的 4 字节 selector。
fn is_relayer_selector() -> [u8; 4] {
    let hash = ethers::utils::keccak256(b"isRelayer(address)");
    [hash[0], hash[1], hash[2], hash[3]]
}

/// eth_call EVM 桥合约的 `isRelayer(address)`，返回 bool。
///
/// 启动期辅助：用最新区块查询（容错性优先），失败由 caller 决定是否 warn。
async fn evm_is_relayer(
    provider: &Provider<Http>,
    contract: Address,
    addr: Address,
) -> Result<bool> {
    let mut calldata = Vec::with_capacity(4 + 32);
    calldata.extend_from_slice(&is_relayer_selector());
    let mut word = [0u8; 32];
    word[12..32].copy_from_slice(addr.as_bytes());
    calldata.extend_from_slice(&word);

    let tx = TypedTransaction::Legacy(
        ethers::types::TransactionRequest::new()
            .to(contract)
            .data(calldata),
    );
    let result = provider
        .call(&tx, None)
        .await
        .context("调用 isRelayer 失败")?;
    if result.len() < 32 {
        bail!("isRelayer 返回值太短: {} 字节", result.len());
    }
    Ok(result[31] != 0)
}

/// 启动期对所有 endpoint 做一次 relayer 白名单核查。
///
/// 行为约定：
/// - 已注册：INFO 一行，便于在日志里确认状态正确；
/// - 未注册：WARN 提示运维去对应链的桥合约把本机地址加进 relayer 列表
///   （否则 confirmEvent / confirm_event 会被合约拒绝，提交侧后续每次都会
///   在 preflight 阶段失败）；
/// - 启动期 RPC 不通 / BridgeState 缺失：WARN 但不 bail —— 与
///   `build_all_endpoints` 的容错策略一致，submitter 启动后会 lazy retry。
///
/// 该检查不阻塞启动；如果某条链的桥合约访问不到，relayer 仍会上线、
/// 由后续真实提交流程兜底报错。
async fn verify_relayer_whitelist(
    endpoints: &[ChainEndpoint],
    svm_pubkey: &Pubkey,
    evm_address: Address,
) {
    for ep in endpoints {
        match ep.kind {
            ChainKind::Svm => {
                // 程序形态：优先从已构造好的 SvmConfig 取（与 submitter 用同一来源），
                // 启动期没拉到 SvmConfig 时回退到注册表。两个来源不会冲突，
                // 因为 build_all_endpoints 本就是从注册表查的形态。
                let kind = ep
                    .svm
                    .as_ref()
                    .map(|c| c.program_kind)
                    .or_else(|| chain_registry::svm_program_kind(ep.chain_id));
                let Some(kind) = kind else {
                    warn!(
                        chain_id = ep.chain_id,
                        "SVM 链未在 chain_registry 配置 svm_program_kind，跳过白名单核查"
                    );
                    continue;
                };
                let rpc = RpcClient::new_with_commitment(
                    ep.rpc_url.clone(),
                    solana_sdk::commitment_config::CommitmentConfig::finalized(),
                );
                let program_id = Pubkey::new_from_array(ep.contract);
                match discovery::fetch_bridge_state(&rpc, &program_id, kind).await {
                    Ok(bs) => {
                        if bs.relayers.contains(svm_pubkey) {
                            info!(
                                chain_id = ep.chain_id,
                                program_kind = %kind,
                                svm_pubkey = %svm_pubkey,
                                "已注册到该 SVM 链 relayer 白名单"
                            );
                        } else {
                            warn!(
                                chain_id = ep.chain_id,
                                program_kind = %kind,
                                svm_pubkey = %svm_pubkey,
                                "未注册到该 SVM 链 relayer 白名单 —— confirm_event 会被拒，请去桥合约添加"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            chain_id = ep.chain_id,
                            program_kind = %kind,
                            "无法验证 SVM relayer 白名单（启动期 RPC 失败，运行时再看 submitter 报错）: {e:#}"
                        );
                    }
                }
            }
            ChainKind::Evm => {
                let contract = match bytes32_to_evm_address(&ep.contract) {
                    Ok(a) => a,
                    Err(e) => {
                        warn!(chain_id = ep.chain_id, "无法解析 EVM 桥合约地址，跳过白名单核查: {e:#}");
                        continue;
                    }
                };
                let provider = match Provider::<Http>::try_from(ep.rpc_url.as_str()) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(chain_id = ep.chain_id, "无法构造 EVM provider 验证白名单: {e:#}");
                        continue;
                    }
                };
                match provider.get_chainid().await {
                    Ok(rpc_chain_id) => {
                        let rpc_id = rpc_chain_id.as_u64();
                        if rpc_id != ep.chain_id {
                            error!(
                                chain_id = ep.chain_id,
                                rpc_chain_id = rpc_id,
                                "EVM 链 chain_id 与 RPC eth_chainId 不一致！EIP-155 签名将被拒"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(chain_id = ep.chain_id, "无法获取 eth_chainId 进行校验: {e:#}");
                    }
                }
                match evm_is_relayer(&provider, contract, evm_address).await {
                    Ok(true) => info!(
                        chain_id = ep.chain_id,
                        evm_address = ?evm_address,
                        "已注册到该 EVM 链 relayer 白名单"
                    ),
                    Ok(false) => warn!(
                        chain_id = ep.chain_id,
                        evm_address = ?evm_address,
                        "未注册到该 EVM 链 relayer 白名单 —— confirmEvent 会被拒，请去桥合约添加"
                    ),
                    Err(e) => warn!(
                        chain_id = ep.chain_id,
                        "无法验证 EVM relayer 白名单（启动期 RPC 失败，运行时再看 submitter 报错）: {e:#}"
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes32_to_evm_address_ok_when_top_zero() {
        let mut b = [0u8; 32];
        b[12..].copy_from_slice(&[0xab; 20]);
        let addr = bytes32_to_evm_address(&b).expect("ok");
        assert_eq!(addr.0, [0xab; 20]);
    }

    #[test]
    fn bytes32_to_evm_address_rejects_top_nonzero() {
        let mut b = [0u8; 32];
        b[0] = 0x01; // 顶部非零
        assert!(bytes32_to_evm_address(&b).is_err());
    }

    #[test]
    fn jittered_submit_interval_within_bounds() {
        for _ in 0..100 {
            let d = jittered_submit_interval();
            let ms = d.as_millis() as u64;
            assert!((SUBMIT_INTERVAL_MIN_MS..=SUBMIT_INTERVAL_MAX_MS).contains(&ms));
        }
    }

    /// EVM poller catchup 判定：模拟 `poll_evm_events` 返回的各种 (from, new_from)
    /// 组合，验证 `evm_should_catch_up` 的边界。
    /// 历史 bug：曾用 `>` 而不是 `>=`，因 delta 上界恰好是 EVM_BLOCK_RANGE，
    /// 永远不会超过 → catchup 恒为 false → CATCHUP_DELAY 长期没启用。
    #[test]
    fn evm_catchup_triggers_only_when_full_page_fetched() {
        let from: u64 = 1000;
        // delta == EVM_BLOCK_RANGE → 拿满整页，继续追
        assert!(evm_should_catch_up(from, from + EVM_BLOCK_RANGE));
        // delta == EVM_BLOCK_RANGE - 1 → 撞 safe_head，停在 POLL_INTERVAL
        assert!(!evm_should_catch_up(from, from + EVM_BLOCK_RANGE - 1));
        // delta == 1 → 只读到 1 个块
        assert!(!evm_should_catch_up(from, from + 1));
        // delta == 0 → from > safe_head 的早返回
        assert!(!evm_should_catch_up(from, from));
        // new_from < from（理论上不应发生）用 saturating_sub 兜底 → false
        assert!(!evm_should_catch_up(from, from - 1));
    }

    /// SVM sig enumerator catchup：返回数量达到 SVM_MAX_SIGS 上限即追赶。
    #[test]
    fn svm_enumerator_catchup_triggers_when_cap_hit() {
        assert!(svm_enumerator_should_catch_up(SVM_MAX_SIGS));
        assert!(svm_enumerator_should_catch_up(SVM_MAX_SIGS + 1));
        assert!(!svm_enumerator_should_catch_up(SVM_MAX_SIGS - 1));
        assert!(!svm_enumerator_should_catch_up(1));
        assert!(!svm_enumerator_should_catch_up(0));
    }

    /// catchup 间隔必须远小于正常轮询间隔，否则 catchup 模式没意义。
    /// 25× 上限（POLL_INTERVAL=5s / CATCHUP_DELAY=200ms = 25）防止后续 PR
    /// 不小心把 CATCHUP_DELAY 调到接近 POLL_INTERVAL。
    #[test]
    fn catchup_delay_is_meaningfully_faster_than_poll_interval() {
        assert!(
            CATCHUP_DELAY * 5 <= POLL_INTERVAL,
            "CATCHUP_DELAY ({:?}) 必须 ≤ POLL_INTERVAL/5 ({:?})，否则 catchup 没意义",
            CATCHUP_DELAY,
            POLL_INTERVAL,
        );
    }

    /// 验证 `format!("{:?}", tx_hash)` 序列化的 tx_hash 能 round-trip
    /// 回原 H256，确保写盘格式与 `parse_tx_hash` 兼容。
    #[test]
    fn tx_hash_debug_roundtrip() {
        let h = ethers::types::H256::from_slice(&[
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
        ]);
        let encoded = format!("{h:?}");
        let parsed = parse_tx_hash(&encoded).expect("parse_tx_hash 必须能解析回去");
        assert_eq!(parsed, h, "Debug 格式 {encoded} 不能 round-trip 回原 H256");
    }
}
