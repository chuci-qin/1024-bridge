//! EVM 事件轮询模块
//!
//! 通过 eth_getLogs 从 EVM 链上获取 StakeEvent 日志。
//! 核心特点：
//! - 只读取 finalized 区块，防止 reorg 导致误处理
//! - 支持限制每次查询的区块范围（适配 Alchemy 等 RPC 的限制）
//! - 首次启动时从 finalized 区块向前回溯指定数量的区块

use anyhow::{Context, Result};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, BlockNumber, Filter, Log, H256};
use tracing::{debug, warn};

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

/// 获取链上 finalized 区块的高度。
///
/// 如果 RPC 不支持 `finalized` 标签（部分测试网），回退到 `latest` 并打印警告。
async fn get_finalized_block_number(provider: &Provider<Http>) -> Result<u64> {
    match provider.get_block(BlockNumber::Finalized).await {
        Ok(Some(block)) => Ok(block
            .number
            .context("finalized 区块缺少 number 字段")?
            .as_u64()),
        Ok(None) | Err(_) => {
            warn!("RPC 不支持 'finalized' 区块标签，回退使用 'latest'");
            Ok(provider
                .get_block_number()
                .await
                .context("获取 latest 区块号")?
                .as_u64())
        }
    }
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
///
/// 返回：
/// - `(events, new_from_block)`：解析出的事件列表和下次查询的起始区块号
///
/// 只查询到 finalized 区块为止，确保不会获取到可能被 reorg 的事件。
pub async fn poll_evm_events(
    provider: &Provider<Http>,
    contract_address: Address,
    from_block: u64,
    max_block_range: u64,
) -> Result<(Vec<StakeEventData>, u64)> {
    // 获取当前 finalized 区块高度
    let finalized = get_finalized_block_number(provider).await?;

    // 如果起始区块已经超过 finalized，说明已追上最新进度
    if from_block > finalized {
        return Ok((vec![], from_block));
    }

    // 限制查询范围，不超过 max_block_range 且不超过 finalized
    let to_block = std::cmp::min(from_block + max_block_range, finalized);

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

/// 首次启动时确定起始扫描区块：从 finalized 区块向前回溯 `scan_back` 个区块。
///
/// 例如 finalized=10000, scan_back=1000 → 从 9000 开始扫描。
/// 使用 saturating_sub 确保不会下溢到负数。
pub async fn initial_from_block(provider: &Provider<Http>, scan_back: u64) -> Result<u64> {
    let finalized = get_finalized_block_number(provider).await?;
    Ok(finalized.saturating_sub(scan_back))
}
