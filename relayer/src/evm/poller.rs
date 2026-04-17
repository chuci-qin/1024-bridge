//! EVM 事件轮询模块
//!
//! 通过 eth_getLogs 从 EVM 链上获取 StakeEvent 日志。
//! 核心特点：
//! - 只读取 safe_head = latest - confirmations 之前的区块，防止 reorg 导致误处理
//! - confirmations 由 chain_registry 按链配置，可被 EVM_CONFIRMATIONS_<chain_id> 覆盖
//! - 支持限制每次查询的区块范围（适配 Alchemy 等 RPC 的限制）
//! - 首次启动时从 safe_head + 1 开始扫描，不回扫历史

use anyhow::{Context, Result};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, Filter, Log, H256};
use tracing::{debug, warn};

use crate::chain_registry;
use crate::types::StakeEventData;

/// 计算 StakeEvent 的 topic0（事件签名的 keccak256 哈希）。
///
/// Solidity 事件：
/// ```solidity
/// event StakeEvent(
///     bytes32 indexed sourceContract,
///     bytes32 indexed targetContract,
///     uint64 sourceChainId,
///     uint64 targetChainId,
///     uint64 blockHeight,
///     uint64 amount,
///     bytes32 sender,
///     bytes32 receiver,
///     uint64 nonce
/// );
/// ```
fn stake_event_topic() -> H256 {
    let sig = "StakeEvent(bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64)";
    H256::from(ethers::utils::keccak256(sig.as_bytes()))
}

/// 获取该链当前的 "safe head"：被认为已经几乎不可能 reorg 的最高区块号。
///
/// 工业标准模型：`safe_head = latest_block - confirmations`，confirmations
/// 由 [`chain_registry::confirmations`] 提供（环境变量 `EVM_CONFIRMATIONS_<chain_id>`
/// 优先于默认硬编码值）。
///
/// 这里**不再**使用 RPC 的 `finalized` 标签：
/// - finalized 协议级 finality 太慢（ETH ~13min、Arb/Base ~10min L1 settle），
///   端到端跨链体验下大多数 dApp 接受不了。
/// - confirmations 是主流跨链桥（Wormhole/LayerZero/Stargate）的稳态档位，
///   配合"target 端也等同样多 confirmations"才能完整防御 reorg。
async fn safe_head_block(provider: &Provider<Http>, chain_id: u64) -> Result<u64> {
    let confs = chain_registry::confirmations(chain_id).with_context(|| {
        format!(
            "未注册的 chain_id={chain_id}：拒绝在缺乏 confirmations 配置的情况下读链\
             （请先在 chain_registry 登记此链，或设置 EVM_CONFIRMATIONS_{chain_id}）"
        )
    })?;
    let latest = provider
        .get_block_number()
        .await
        .context("查询 latest 区块号失败")?
        .as_u64();
    Ok(latest.saturating_sub(confs))
}

/// 从 EVM 日志条目中解析 StakeEvent。
///
/// EVM 日志布局：
/// - topics[0]：事件签名哈希（已被 filter 匹配）
/// - topics[1]：sourceContract（indexed bytes32）
/// - topics[2]：targetContract（indexed bytes32）
/// - data：ABI 编码的非索引字段（7 个 32 字节的 word）
///   - word[0]：sourceChainId（uint64，右对齐在 32 字节中）
///   - word[1]：targetChainId
///   - word[2]：blockHeight
///   - word[3]：amount
///   - word[4]：sender（bytes32）
///   - word[5]：receiver（bytes32）
///   - word[6]：nonce
pub fn parse_stake_event(log: &Log) -> Result<StakeEventData> {
    if log.topics.len() < 3 {
        anyhow::bail!("StakeEvent 需要至少 3 个 topics");
    }

    // 从 indexed topics 中提取 sourceContract 和 targetContract
    let source_contract: [u8; 32] = log.topics[1].into();
    let target_contract: [u8; 32] = log.topics[2].into();

    let data = &log.data.0;
    if data.len() < 7 * 32 {
        anyhow::bail!(
            "StakeEvent data 太短: {} 字节，期望 >= {} 字节",
            data.len(),
            7 * 32
        );
    }

    /// 从 ABI 编码的 32 字节 word 中读取 uint64（大端序，右对齐）
    fn read_u64(slice: &[u8], offset: usize) -> u64 {
        let word = &slice[offset..offset + 32];
        u64::from_be_bytes(word[24..32].try_into().unwrap())
    }

    /// 从 ABI 编码中读取一个完整的 bytes32
    fn read_bytes32(slice: &[u8], offset: usize) -> [u8; 32] {
        let mut out = [0u8; 32];
        out.copy_from_slice(&slice[offset..offset + 32]);
        out
    }

    let source_chain_id = read_u64(data, 0);       // word[0]
    let target_chain_id = read_u64(data, 32);       // word[1]
    let block_height = read_u64(data, 64);          // word[2]
    let amount = read_u64(data, 96);                // word[3]
    let sender = read_bytes32(data, 128);           // word[4]
    let receiver = read_bytes32(data, 160);         // word[5]
    let nonce = read_u64(data, 192);                // word[6]

    Ok(StakeEventData {
        source_contract,
        target_contract,
        source_chain_id,
        target_chain_id,
        block_height,
        amount,
        sender,
        receiver,
        nonce,
    })
}

/// 轮询 EVM 链上的 StakeEvent 日志。
///
/// 参数：
/// - `contract_address`：桥合约地址
/// - `from_block`：起始区块号（inclusive）
/// - `max_block_range`：单次查询的最大区块范围（如 Alchemy 限制 10 块）
/// - `chain_id`：用于按链查找 confirmations（reorg 防护深度）
///
/// 返回：
/// - `(events, new_from_block)`：解析出的事件列表和下次查询的起始区块号
///
/// 只查询到 `latest - confirmations` 为止，确保不会读到极易被 reorg 的事件。
pub async fn poll_evm_events(
    provider: &Provider<Http>,
    contract_address: Address,
    from_block: u64,
    max_block_range: u64,
    chain_id: u64,
) -> Result<(Vec<StakeEventData>, u64)> {
    let safe_head = safe_head_block(provider, chain_id).await?;

    // 如果起始区块已经超过 safe_head，说明已追上最新进度
    if from_block > safe_head {
        return Ok((vec![], from_block));
    }

    // 限制查询范围，不超过 max_block_range 且不超过 safe_head
    let to_block = std::cmp::min(from_block + max_block_range, safe_head);

    // 构建 eth_getLogs 的过滤器
    let filter = Filter::new()
        .address(contract_address)          // 只看桥合约的日志
        .topic0(stake_event_topic())        // 只匹配 StakeEvent 事件
        .from_block(from_block)
        .to_block(to_block);

    let logs = provider.get_logs(&filter).await.context("eth_getLogs 调用失败")?;

    let mut events = Vec::new();
    for log in &logs {
        match parse_stake_event(log) {
            Ok(event) => {
                debug!(
                    nonce = event.nonce,
                    amount = event.amount,
                    block = ?log.block_number,
                    "解析到 EVM StakeEvent"
                );
                events.push(event);
            }
            Err(e) => {
                warn!(tx = ?log.transaction_hash, "解析 StakeEvent 失败: {e}");
            }
        }
    }

    // 下次从 to_block + 1 开始扫描
    Ok((events, to_block + 1))
}

/// 首次启动时确定起始扫描区块：从下一个 safe_head 之后开始（不回扫历史）。
///
/// M7 调整：原先固定回扫 1000 块对不同链时间窗口差异巨大（ETH ≈ 3.3h，
/// Arbitrum ≈ 4min）。现在统一不回扫，因为：
/// - checkpoint + 待处理事件都已持久化，正常重启不会丢任何东西；
/// - 第一次部署上线时，运维要么先准备好数据，要么手动改 checkpoint 文件。
///
/// 返回值：`safe_head + 1`，即下一轮 poll 等待新区块产生后再开始处理。
pub async fn initial_from_block(provider: &Provider<Http>, chain_id: u64) -> Result<u64> {
    let safe_head = safe_head_block(provider, chain_id).await?;
    Ok(safe_head.saturating_add(1))
}

// ─────────────────────────────────────────────────────────────────────────────
// 单元测试（L7）
// 这些测试只覆盖纯函数（事件签名、日志解析），不联网。
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::{Bytes, H160, U256, U64};

    /// 构造一条与 EVM 合约 emit 出来时同等格式的 Log，用于解析回测。
    /// 字段的 ABI 编码规则：bytes32 占整个 word；uint64 占 word 末尾 8B（大端右对齐）。
    fn build_log(event: &StakeEventData) -> Log {
        let mut data = Vec::with_capacity(7 * 32);
        let mut u64_word = [0u8; 32];

        u64_word[24..].copy_from_slice(&event.source_chain_id.to_be_bytes());
        data.extend_from_slice(&u64_word);
        u64_word = [0u8; 32];
        u64_word[24..].copy_from_slice(&event.target_chain_id.to_be_bytes());
        data.extend_from_slice(&u64_word);
        u64_word = [0u8; 32];
        u64_word[24..].copy_from_slice(&event.block_height.to_be_bytes());
        data.extend_from_slice(&u64_word);
        u64_word = [0u8; 32];
        u64_word[24..].copy_from_slice(&event.amount.to_be_bytes());
        data.extend_from_slice(&u64_word);
        data.extend_from_slice(&event.sender);
        data.extend_from_slice(&event.receiver);
        u64_word = [0u8; 32];
        u64_word[24..].copy_from_slice(&event.nonce.to_be_bytes());
        data.extend_from_slice(&u64_word);

        Log {
            address: H160::zero(),
            topics: vec![
                stake_event_topic(),
                H256::from(event.source_contract),
                H256::from(event.target_contract),
            ],
            data: Bytes::from(data),
            block_hash: None,
            block_number: Some(U64::from(123)),
            transaction_hash: None,
            transaction_index: None,
            log_index: Some(U256::from(0)),
            transaction_log_index: None,
            log_type: None,
            removed: Some(false),
        }
    }

    fn sample_event() -> StakeEventData {
        StakeEventData {
            source_contract: [0x11; 32],
            target_contract: [0x22; 32],
            source_chain_id: 1,
            target_chain_id: 91024,
            block_height: 0xcafe,
            amount: 12345,
            sender: [0x33; 32],
            receiver: [0x44; 32],
            nonce: 7,
        }
    }

    /// 构造 Log → 解析 → 字段必须 1:1 一致。
    /// 这条 round-trip 是 EVM 端事件解析正确性的核心保证。
    #[test]
    fn parse_stake_event_roundtrip() {
        let original = sample_event();
        let log = build_log(&original);
        let parsed = parse_stake_event(&log).expect("parse ok");
        assert_eq!(parsed, original);
    }

    /// topic0 必须严格等于事件签名的 keccak256 哈希。
    /// 改了 Solidity event 签名却忘了同步这里时，eth_getLogs 会返回 0 条结果。
    #[test]
    fn stake_event_topic_matches_keccak() {
        let sig = "StakeEvent(bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64)";
        let expected = H256::from(ethers::utils::keccak256(sig.as_bytes()));
        assert_eq!(stake_event_topic(), expected);
    }

    /// topics 不足 3 项时（缺少 source/target contract）应返回 Err，而不是 panic。
    #[test]
    fn parse_stake_event_rejects_missing_topics() {
        let mut log = build_log(&sample_event());
        log.topics.truncate(2);
        assert!(parse_stake_event(&log).is_err());
    }

    /// data 不足 7×32B 时应返回 Err，而不是越界 panic。
    #[test]
    fn parse_stake_event_rejects_short_data() {
        let mut log = build_log(&sample_event());
        log.data = Bytes::from(vec![0u8; 100]);
        assert!(parse_stake_event(&log).is_err());
    }
}
