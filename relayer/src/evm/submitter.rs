//! EVM 确认交易提交模块（pipelined submit + async confirmation 模型）。
//!
//! 负责三件事：
//! 1. [`check_nonce_status`] —— 用 `getNonceStatus` 视图查 nonce 是否已上链
//! 2. [`broadcast_confirm_event`] —— 广播 `confirmEvent` tx，**不等回执**，
//!    立即返回 tx_hash 让上层把 submission 写盘
//! 3. [`check_tx_maturity`] —— 给定 tx_hash，查询其链上成熟度（够 N confs / 等待中 / revert / 找不到）
//!
//! 这三件事被 `main::process_event_for_evm` 串起来，使得：
//! - 单事件每轮只花 ~RPC 延迟而非 ~N×blocktime（旧 12 块 = 2.4min → 新 200ms）
//! - 多事件可以"在飞"等成熟，互不阻塞
//!
//! ABI 编码说明：
//! - EVM 的 uint64 在 ABI 中占一个 32 字节 word，值大端序右对齐
//! - bytes32 直接占一个 32 字节 word

use anyhow::{Context, Result};
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::transaction::eip1559::Eip1559TransactionRequest;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, BlockId, BlockNumber, TransactionRequest, TxHash, U256};
use tracing::{info, warn};

use crate::chain_registry;
use crate::types::StakeEventData;

/// EVM 签名 + 广播客户端的别名：把 `LocalWallet` 包到 `Provider<Http>` 上，
/// 让一次性签名/估算 gas/取 nonce 都走同一个 middleware 栈。
///
/// 构造一次后即可在多笔 tx 之间复用：内部不缓存 nonce，每次 send_transaction
/// 都会重新 `eth_getTransactionCount(pending)` 取最新 nonce。
pub type EvmClient = SignerMiddleware<Provider<Http>, LocalWallet>;

/// 控制 `check_nonce_status` 在哪个块高上做 eth_call。
pub enum NonceCheckBlock {
    /// 直接使用 latest_block（即时状态，用于 Branch A step1 快速感知）
    Latest,
    /// 退缩 confs 个块（safe_head = latest - confs，用于最终确认/防 reorg）
    SafeHead,
}

/// 一笔已广播 tx 的链上成熟度状态。供上层状态机决策用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxMaturity {
    /// tx 已 mined 且达到 N confirmations，可视为最终。返回 mined block 号。
    Confirmed { mined_block: u64 },
    /// tx 已 mined 但确认数还不够。返回 mined block 号（缓存以避免下轮重复拉 receipt）。
    Pending { mined_block: u64, current_depth: u64 },
    /// receipt 还没出现：可能仍在 mempool，也可能被 reorg 出去 / 被替换 / 被 drop。
    /// 上层结合 sent_at_unix 做 stale 判断。
    NotYetMined,
    /// receipt 出现了但 status=0：链上 revert（多见于 RelayerNotFound / AlreadyProcessed）。
    /// `gas_used` 用于诊断：若 gas_used ≈ gas_limit 则是 Out of Gas，否则是合约 require 失败。
    Reverted { mined_block: u64, gas_used: u64 },
}

/// 计算 confirmEvent 函数的 4 字节选择器。
///
/// Solidity 函数签名：
/// `confirmEvent((bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64))`
///
/// 选择器 = keccak256(函数签名) 的前 4 字节。
fn confirm_event_selector() -> [u8; 4] {
    let sig = "confirmEvent((bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64))";
    let hash = ethers::utils::keccak256(sig.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// 计算 getNonceStatus 函数的 4 字节选择器。
///
/// Solidity 函数签名：`getNonceStatus(uint64,address)`
/// 返回值：`(bool isProcessed, bool relayerConfirmed)`
fn get_nonce_status_selector() -> [u8; 4] {
    let sig = "getNonceStatus(uint64,address)";
    let hash = ethers::utils::keccak256(sig.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// 将 StakeEventData ABI 编码为 confirmEvent 的 calldata。
///
/// 编码布局（共 4 + 9×32 = 292 字节）：
/// ```text
/// [4B 选择器]
/// [32B sourceContract]     ← bytes32
/// [32B targetContract]     ← bytes32
/// [32B sourceChainId]      ← uint64, 大端序右对齐
/// [32B targetChainId]      ← uint64
/// [32B blockHeight]        ← uint64
/// [32B amount]             ← uint64
/// [32B sender]             ← bytes32
/// [32B receiver]           ← bytes32
/// [32B nonce]              ← uint64
/// ```
fn encode_confirm_event(event: &StakeEventData) -> Vec<u8> {
    let mut calldata = Vec::with_capacity(4 + 9 * 32);
    calldata.extend_from_slice(&confirm_event_selector());

    // bytes32 字段直接写入
    calldata.extend_from_slice(&event.source_contract);
    calldata.extend_from_slice(&event.target_contract);

    // uint64 字段：放在 32 字节 word 的后 8 字节，前 24 字节为 0
    let mut word = [0u8; 32];
    word[24..32].copy_from_slice(&event.source_chain_id.to_be_bytes());
    calldata.extend_from_slice(&word);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.target_chain_id.to_be_bytes());
    calldata.extend_from_slice(&word);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.block_height.to_be_bytes());
    calldata.extend_from_slice(&word);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.amount.to_be_bytes());
    calldata.extend_from_slice(&word);

    calldata.extend_from_slice(&event.sender);
    calldata.extend_from_slice(&event.receiver);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.nonce.to_be_bytes());
    calldata.extend_from_slice(&word);

    calldata
}

/// EVM 侧 nonce 状态，与 SVM 的 `NonceStatus` 对齐。
///
/// 合并了"是否已处理"和"本 relayer 是否已投票"两个维度，
/// 两次 `eth_call`（`nonceConfirmations` + `hasRelayerConfirmed`）即可完成决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonceStatus {
    /// `isProcessed == true` → 事件已完全处理（投票达阈值并已 unlock）
    FullyProcessed,
    /// `isProcessed == false`，但本 relayer 已投过票 →
    /// 无需重复广播，等待其他 relayer 投票达到阈值
    AlreadyConfirmedByUs,
    /// `isProcessed == false`，且本 relayer 未投票 → 需要广播 confirmEvent
    PendingOurVote,
}

/// 检查某个 nonce 在 EVM 链上的状态，同时判断指定 relayer 是否已投票。
///
/// 单次 `eth_call` 调用合约的 `getNonceStatus(uint64, address)` 视图函数，
/// 返回 `(bool isProcessed, bool relayerConfirmed)`，一次 RPC 即可完成决策。
///
/// `block_sel` 控制查询块高：
/// - `Latest`：直接用 `latest_block`，用于 Branch A step1 快速感知
/// - `SafeHead`：退缩 confs 个块（`latest - confs`），用于最终确认/删文件
///
/// `latest_block` 由 caller 提供（通常 submitter 每轮拉一次复用），避免每个事件都
/// 重新调用 `eth_blockNumber`。
pub async fn check_nonce_status(
    provider: &Provider<Http>,
    contract: Address,
    chain_id: u64,
    nonce: u64,
    relayer: Address,
    latest_block: u64,
    block_sel: NonceCheckBlock,
) -> Result<NonceStatus> {
    let at_block = match block_sel {
        NonceCheckBlock::Latest => latest_block,
        NonceCheckBlock::SafeHead => {
            let confs = chain_registry::confirmations(chain_id).with_context(|| {
                format!(
                    "未注册的 chain_id={chain_id}：拒绝在缺乏 confirmations 配置的情况下查 nonce 状态"
                )
            })?;
            latest_block.saturating_sub(confs)
        }
    };
    let block: BlockId = BlockNumber::Number(at_block.into()).into();

    let mut calldata = Vec::with_capacity(4 + 64);
    calldata.extend_from_slice(&get_nonce_status_selector());
    let mut word = [0u8; 32];
    word[24..32].copy_from_slice(&nonce.to_be_bytes());
    calldata.extend_from_slice(&word);
    let mut word = [0u8; 32];
    word[12..32].copy_from_slice(relayer.as_bytes());
    calldata.extend_from_slice(&word);

    let tx = TypedTransaction::Legacy(TransactionRequest::new().to(contract).data(calldata));
    let result = provider
        .call(&tx, Some(block))
        .await
        .context("调用 getNonceStatus 失败")?;

    if result.len() < 64 {
        anyhow::bail!("getNonceStatus 返回值太短: {} 字节（需要 64）", result.len());
    }

    let is_processed = result[31] != 0;
    let relayer_confirmed = result[63] != 0;

    if is_processed {
        Ok(NonceStatus::FullyProcessed)
    } else if relayer_confirmed {
        Ok(NonceStatus::AlreadyConfirmedByUs)
    } else {
        Ok(NonceStatus::PendingOurVote)
    }
}

/// 广播 confirmEvent 交易到 EVM 链，**不等待任何回执**，立即返回 tx_hash。
///
/// 使用 EIP-1559 交易类型：`fill_transaction` 自动从 `eth_feeHistory` 填充
/// `max_fee_per_gas`（≈ baseFee×2 + priority_fee）和 `max_priority_fee_per_gas`，
/// 能扛住连续 6 个块的最大速率 baseFee 上涨，大幅减少因 gas 不足卡在 mempool 的概率。
///
/// 流程：
/// 1. `client` (SignerMiddleware) 自动估算 gas、取 pending nonce、签名、广播
/// 2. 拿到 PendingTransaction 后立刻取出 tx_hash 并丢弃 future（tx 已在 mempool）
/// 3. 上层把 (tx_hash, sent_at_unix) 写到事件文件的 submission 字段
///
/// 后续等待确认 / 检测 reorg 由 [`check_tx_maturity`] 在后续轮次完成。
pub async fn broadcast_confirm_event(
    client: &EvmClient,
    contract: Address,
    chain_id: u64,
    event: &StakeEventData,
) -> Result<TxHash> {
    let calldata = encode_confirm_event(event);
    let tx = Eip1559TransactionRequest::new().to(contract).data(calldata);
    let mut typed_tx: TypedTransaction = tx.into();

    // eth_estimateGas 基于当前状态，但打包时可能走更贵的路径
    // （例如其他 relayer 的票先落地，本 tx 变成达阈值票 + safeTransfer），
    // 加 20% buffer 防止 OOG。
    let estimated = client
        .estimate_gas(&typed_tx, None)
        .await
        .context("估算 confirmEvent gas 失败")?;
    typed_tx.set_gas(estimated * 6 / 5);

    let pending = client
        .send_transaction(typed_tx, None)
        .await
        .context("发送 confirmEvent 交易失败")?;

    let tx_hash = pending.tx_hash();
    info!(
        nonce = event.nonce,
        source_chain_id = event.source_chain_id,
        target_chain_id = chain_id,
        tx_hash = ?tx_hash,
        "已广播 EVM confirmEvent EIP-1559 交易（不等回执，下一轮检查成熟度）"
    );
    drop(pending);
    Ok(tx_hash)
}

/// 检查一笔已广播但未 mined 的 tx 是否仍存活在 mempool 中。
///
/// 在 NotYetMined + stale 的场景下用于决定是清 submission 还是 self-transfer：
/// - `Some(tx)` 且 `block_number.is_none()` → 仍在 mempool，需要 self-transfer 顶替
/// - `Some(tx)` 且 `block_number.is_some()` → 刚在竞态中上链，下轮 check_tx_maturity 处理
/// - `None` → mempool 已 evict，直接清 submission 让 Branch A 重广播
pub async fn get_pending_transaction(
    provider: &Provider<Http>,
    tx_hash: TxHash,
) -> Result<Option<ethers::types::Transaction>> {
    provider
        .get_transaction(tx_hash)
        .await
        .context("查询 mempool 中的 tx 失败")
}

/// 发一笔 self-transfer（to=自己, value=0, EIP-1559）顶替 mempool 中卡住的 stale tx，
/// 强行推进账户的 state nonce 让后续 confirm tx 能继续上链。
///
/// 只需传入要顶替的 EVM 账户 nonce，fee 由 `fill_transaction` 从 `eth_feeHistory`
/// 自动填充当前网络最优值，不依赖旧 tx 的 gas_price。
///
/// self-transfer 的特性：
/// - `to = 自己地址`，`value = 0`，`data` 空 → 不触发任何合约逻辑 → 永远不会 revert
/// - `gas = 21000`：纯转账固定消耗，显式写死跳过 estimateGas
///
/// 调用方拿到 Ok 后应清 submission，交给下轮 Branch A 统一决策。
pub async fn send_self_transfer_to_unblock(
    client: &EvmClient,
    nonce: U256,
) -> Result<TxHash> {
    let self_addr = client.signer().address();

    let tx = Eip1559TransactionRequest::new()
        .to(self_addr)
        .value(U256::zero())
        .nonce(nonce)
        .gas(U256::from(21_000u64));

    let pending = client
        .send_transaction(tx, None)
        .await
        .context("广播 self-transfer 失败（余额不足 / 节点拒绝）")?;
    let new_hash = pending.tx_hash();
    warn!(
        nonce = nonce.as_u64(),
        self_transfer_hash = ?new_hash,
        "已发 EIP-1559 self-transfer 顶替 mempool 中的 stale tx 以推进 nonce"
    );
    drop(pending);
    Ok(new_hash)
}

/// 查询一笔已广播 tx 的成熟度。每轮 submitter 用 0-1 次 RPC 调用即可推进状态机：
/// - 没看到 receipt → `NotYetMined`（上层结合 sent_at 做 stale 处理）
/// - 看到 receipt 但 confirmations 还不够 → `Pending`（caller 应缓存返回的 mined_block）
/// - 看到 receipt 且 confirmations > N → `Confirmed`
/// - 看到 receipt 但 status=0 → `Reverted`
///
/// "1 confirmation" 定义按 ethers 惯例：tx 所在块即为第 1 个 confirmation，
/// 即 `depth = latest_block - mined_block + 1`。
///
/// `latest_block` 由 caller 提供（通常 submitter 每轮拉一次复用）。落后于真实链 head
/// 只会让 depth 偏小一点点 → 偶尔多等 1 轮才确认成熟，绝不会误判 unconfirmed 为 Confirmed。
///
/// ## Fast path：`cached_mined_block`
///
/// 当 caller（submitter）已经在前一轮看到过 `Pending { mined_block }` 并把它缓存到
/// `Submission.mined_block` 时，本轮**直接按 `latest - cached + 1` 算 depth**，
/// 跳过 `eth_getTransactionReceipt`：
/// - `depth > confs` → `Confirmed { mined_block: cached }`
/// - 否则 → `Pending { mined_block: cached, current_depth }`
///
/// ETH 主网 confs=12 + ~12s blocktime + 1.5s 扫描周期下，单笔 tx 在
/// confs 等待期内能省 ~96 次 receipt RPC。
///
/// ### Fast path 的安全性依赖
///
/// fast-path 不会重新读取 receipt，所以**感知不到** Pending 期间的 reorg / status 变化
/// （例如 mined → reorg evict → 新分支 status=0）。这是有意为之，原因如下：
/// - `process_evm_entry` 在 `Confirmed` 分支必然再调一次 `check_nonce_status`
///   做兜底校验（在 `safe_head = latest - confs` 上 eth_call）。
/// - 如果 reorg 导致 tx 实际未生效，`check_nonce_status` 返回 `PendingOurVote` →
///   清 submission 重广播，没有错误删文件 / 双花风险。
/// - 唯一代价是异常感知会延迟到 `confs * blocktime`（ETH 主网 ~2.4min），
///   但这种 reorg evict 在生产中极少出现，性价比合算。
///
/// **caller 必须保留 Confirmed 分支的二次 `check_nonce_status`**，否则 fast-path
/// 不安全。
pub async fn check_tx_maturity(
    provider: &Provider<Http>,
    chain_id: u64,
    tx_hash: TxHash,
    latest_block: u64,
    cached_mined_block: Option<u64>,
) -> Result<TxMaturity> {
    let confs = chain_registry::confirmations(chain_id).with_context(|| {
        format!(
            "未注册的 chain_id={chain_id}：拒绝在缺乏 confirmations 配置的情况下检查 tx 成熟度"
        )
    })?;

    // ── Fast path：复用前一轮缓存的 mined_block，跳过 receipt 调用 ──
    if let Some(mined_block) = cached_mined_block {
        let depth = latest_block.saturating_sub(mined_block).saturating_add(1);
        if depth > confs {
            return Ok(TxMaturity::Confirmed { mined_block });
        }
        return Ok(TxMaturity::Pending { mined_block, current_depth: depth });
    }

    // ── Slow path：第一次见到这笔 tx，必须拉 receipt ──
    let receipt = provider
        .get_transaction_receipt(tx_hash)
        .await
        .context("查询 transaction receipt 失败")?;
    let Some(r) = receipt else {
        return Ok(TxMaturity::NotYetMined);
    };

    let mined_block = r
        .block_number
        .context("receipt 缺少 block_number 字段")?
        .as_u64();
    let status = r
        .status
        .context("receipt 缺少 status 字段（pre-Byzantium 链不支持）")?;
    if status.as_u64() != 1 {
        let gas_used = r.gas_used.map(|g| g.as_u64()).unwrap_or(0);
        return Ok(TxMaturity::Reverted { mined_block, gas_used });
    }

    // depth > confs 而非 >= confs，与 check_nonce_status 的 safe_head = latest - confs 对齐：
    // 当 depth = confs + 1 时，mined_block <= latest - confs，safe_head 必然 >= mined_block。
    let depth = latest_block.saturating_sub(mined_block).saturating_add(1);
    if depth > confs {
        Ok(TxMaturity::Confirmed { mined_block })
    } else {
        Ok(TxMaturity::Pending { mined_block, current_depth: depth })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> StakeEventData {
        StakeEventData {
            source_contract: [0xaa; 32],
            target_contract: [0xbb; 32],
            source_chain_id: 91024,
            target_chain_id: 1,
            block_height: 7,
            amount: 1_000,
            sender: [0xcc; 32],
            receiver: [0xdd; 32],
            nonce: 99,
        }
    }

    /// 选择器是 keccak256(签名)[..4]，纯函数，结果应稳定。
    /// 重新计算一次并比对，防止有人误改函数签名字符串。
    #[test]
    fn confirm_event_selector_is_keccak_prefix() {
        let sig = "confirmEvent((bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64))";
        let expected = ethers::utils::keccak256(sig.as_bytes());
        let got = confirm_event_selector();
        assert_eq!(&got[..], &expected[..4]);
    }

    #[test]
    fn get_nonce_status_selector_is_keccak_prefix() {
        let expected = ethers::utils::keccak256("getNonceStatus(uint64,address)".as_bytes());
        let got = get_nonce_status_selector();
        assert_eq!(&got[..], &expected[..4]);
    }

    /// confirmEvent calldata 长度 = 4B 选择器 + 9 个 32B word = 292 字节。
    /// 任何 ABI 编码错误（漏字段、多字段、错对齐）都会让长度变化。
    #[test]
    fn encode_confirm_event_has_correct_length() {
        let calldata = encode_confirm_event(&sample_event());
        assert_eq!(calldata.len(), 4 + 9 * 32);
    }

    /// 字段级正确性：取出每个 32B word 校验内容是否符合 ABI 规则。
    /// - bytes32 直接 32B
    /// - uint64 在 word 末尾 8B 大端序，前 24B 必须为 0
    #[test]
    fn encode_confirm_event_layout_is_correct() {
        let ev = sample_event();
        let calldata = encode_confirm_event(&ev);

        // 跳过 4B 选择器
        let body = &calldata[4..];
        let word = |i: usize| &body[i * 32..(i + 1) * 32];

        // word 0/1: source_contract / target_contract（bytes32）
        assert_eq!(word(0), &ev.source_contract[..]);
        assert_eq!(word(1), &ev.target_contract[..]);

        // word 2: source_chain_id（uint64 BE 右对齐）
        assert!(word(2)[..24].iter().all(|b| *b == 0), "前 24B 必须零填充");
        assert_eq!(&word(2)[24..32], &ev.source_chain_id.to_be_bytes());

        // word 3..6: target_chain_id, block_height, amount —— 同样规则
        assert_eq!(&word(3)[24..32], &ev.target_chain_id.to_be_bytes());
        assert_eq!(&word(4)[24..32], &ev.block_height.to_be_bytes());
        assert_eq!(&word(5)[24..32], &ev.amount.to_be_bytes());

        // word 6/7: sender / receiver（bytes32）
        assert_eq!(word(6), &ev.sender[..]);
        assert_eq!(word(7), &ev.receiver[..]);

        // word 8: nonce
        assert!(word(8)[..24].iter().all(|b| *b == 0));
        assert_eq!(&word(8)[24..32], &ev.nonce.to_be_bytes());
    }

    /// 选择器在 calldata 头部的位置正确。
    #[test]
    fn encode_confirm_event_selector_first() {
        let calldata = encode_confirm_event(&sample_event());
        assert_eq!(&calldata[..4], &confirm_event_selector()[..]);
    }

    /// fast-path：cached_mined_block 提供时，**不调用 provider**，按 latest - cached + 1
    /// 算 depth 直接返回 Pending / Confirmed。
    ///
    /// 关键保证：fast-path 永远不返回 NotYetMined / Reverted —— 这两种状态只能由 receipt
    /// 给出，fast-path 跳过了 receipt 调用。Reverted 在 process_evm_entry 的 Confirmed
    /// 分支由 check_nonce_status 兜底校验感知。
    ///
    /// 这条测试用一个**指向不可达地址**的 provider：如果实现意外调了网络，会因为
    /// connection refused 而 Err（`expect("ok")` 失败）。fast-path 不打网络则永远 ok。
    #[tokio::test]
    async fn fast_path_skips_receipt_call_and_returns_pending_below_confs() {
        // chain_id=1（ETH），confs=12
        let provider = Provider::<Http>::try_from("http://127.0.0.1:1") // 不可达
            .expect("provider build");
        // mined=100, latest=105 → depth=6 < 12 → Pending
        let m = check_tx_maturity(&provider, 1, TxHash::zero(), 105, Some(100))
            .await
            .expect("fast-path 不打网络，必须 Ok");
        assert_eq!(
            m,
            TxMaturity::Pending {
                mined_block: 100,
                current_depth: 6,
            }
        );
    }

    #[tokio::test]
    async fn fast_path_returns_confirmed_when_depth_exceeds_confs() {
        let provider = Provider::<Http>::try_from("http://127.0.0.1:1").expect("provider build");
        // mined=100, latest=112 → depth=13 > 12 → Confirmed
        let m = check_tx_maturity(&provider, 1, TxHash::zero(), 112, Some(100))
            .await
            .expect("fast-path 必须 Ok");
        assert_eq!(m, TxMaturity::Confirmed { mined_block: 100 });
    }

    /// 极端 case：latest 短暂落后 mined（RPC 节点之间不一致 / 节点切换）。
    /// saturating_sub 保证不下溢，depth = 1（视为本块即第 1 个 confirmation）。
    #[tokio::test]
    async fn fast_path_saturates_when_latest_below_mined() {
        let provider = Provider::<Http>::try_from("http://127.0.0.1:1").expect("provider build");
        // latest < mined：很罕见但不应 panic / 误判 Confirmed
        let m = check_tx_maturity(&provider, 1, TxHash::zero(), 95, Some(100))
            .await
            .expect("fast-path 必须 Ok");
        assert_eq!(
            m,
            TxMaturity::Pending {
                mined_block: 100,
                current_depth: 1,
            }
        );
    }

    /// 边界：depth 恰好 = confs 时仍是 Pending，depth = confs + 1 才是 Confirmed。
    /// 这与 check_nonce_status 的 safe_head = latest - confs 对齐。
    #[tokio::test]
    async fn fast_path_boundary_at_exact_confs() {
        let provider = Provider::<Http>::try_from("http://127.0.0.1:1").expect("provider build");

        // depth=12 = confs → 仍是 Pending（需要 > confs 才 Confirmed）
        let m = check_tx_maturity(&provider, 1, TxHash::zero(), 111, Some(100))
            .await
            .expect("ok");
        assert!(matches!(m, TxMaturity::Pending { current_depth: 12, .. }));

        // depth=13 > 12 → Confirmed
        let m = check_tx_maturity(&provider, 1, TxHash::zero(), 112, Some(100))
            .await
            .expect("ok");
        assert!(matches!(m, TxMaturity::Confirmed { mined_block: 100 }));
    }

    /// fast-path 在未注册 chain_id 上必须 fail-fast，与 slow-path 一致 ——
    /// 否则会用错误的 confs 误判成熟度。
    #[tokio::test]
    async fn fast_path_rejects_unregistered_chain_id() {
        let provider = Provider::<Http>::try_from("http://127.0.0.1:1").expect("provider build");
        let r = check_tx_maturity(&provider, /* 未注册 */ 99_999_999, TxHash::zero(), 200, Some(100)).await;
        assert!(r.is_err(), "未注册 chain_id 必须 Err");
    }

}
