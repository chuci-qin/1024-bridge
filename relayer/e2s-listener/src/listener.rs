use crate::config::ListenerConfig;
use anyhow::{anyhow, Result};
use ethers::{
    abi::ParamType,
    contract::EthEvent,
    core::types::Address,
    prelude::*,
    providers::{Http, Middleware, Provider},
};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};
use shared::types::StakeEventData;
use std::{path::Path, sync::Arc};
use tracing::{debug, error, info, warn};

// StakeEvent ABI 定义
#[derive(Debug, Clone, EthEvent)]
#[ethevent(
    name = "StakeEvent",
    abi = "StakeEvent(bytes32,bytes32,uint64,uint64,uint64,address,string,uint64)"
)]
pub struct StakeEvent {
    #[ethevent(indexed)]
    pub source_contract: [u8; 32],
    #[ethevent(indexed)]
    pub target_contract: [u8; 32],
    pub chain_id: u64,
    pub block_height: u64,
    pub amount: u64,
    pub sender: Address,            // EVM 发起者地址
    pub receiver_address: String,   // Solana 接收地址
    pub nonce: u64,
}

/// 启动 EVM 事件监听器
pub async fn start_listener(config: ListenerConfig) -> Result<()> {
    info!("Starting EVM event listener");
    info!(
        rpc = config.source_chain.rpc_url,
        contract = config.source_chain.contract_address,
        "Connecting to EVM"
    );

    // 创建 Provider
    let provider = Provider::<Http>::try_from(&config.source_chain.rpc_url)
        .map_err(|e| anyhow!("Failed to create provider: {}", e))?;
    let provider = Arc::new(provider);

    // 解析合约地址
    let contract_address: Address = config
        .source_chain
        .contract_address
        .parse()
        .map_err(|e| anyhow!("Invalid contract address: {}", e))?;

    info!("Connected to EVM, starting to listen for events");

    let queue_dir = &config.queue.path;
    std::fs::create_dir_all(queue_dir)?;
    info!(queue_path = %queue_dir.display(), "Queue directory initialized");

    let checkpoint_path = queue_dir.join("checkpoint.json");

    let mut last_block = if let Ok(start) = std::env::var("START_BLOCK") {
        let block = start.parse::<u64>()
            .map_err(|_| anyhow!("Invalid START_BLOCK: {}", start))?;
        info!(block, "Starting from START_BLOCK override");
        block
    } else if let Some(block) = load_checkpoint(&checkpoint_path) {
        info!(block, "Resuming from checkpoint");
        block
    } else {
        let block = provider
            .get_block_number()
            .await
            .map_err(|e| anyhow!("Failed to get block number: {}", e))?
            .as_u64();
        info!(block, "Starting from current block (first run)");
        block
    };

    loop {
        match listen_for_events(&provider, contract_address, last_block, &config).await {
            Ok(new_block) => {
                if new_block > last_block {
                    last_block = new_block;
                    if let Err(e) = save_checkpoint(&checkpoint_path, last_block) {
                        error!(error = %e, "Failed to save checkpoint");
                    }
                }
            }
            Err(e) => {
                error!("Error listening for events: {}", e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

/// 监听指定区块范围的事件
async fn listen_for_events(
    provider: &Provider<Http>,
    contract_address: Address,
    from_block: u64,
    config: &ListenerConfig,
) -> Result<u64> {
    // 获取最新区块号
    let latest_block = provider
        .get_block_number()
        .await
        .map_err(|e| anyhow!("Failed to get latest block: {}", e))?
        .as_u64();

    // 如果没有新区块，返回当前区块号
    if latest_block <= from_block {
        return Ok(from_block);
    }

    // 查询事件（限制查询范围以避免超时）
    let to_block = std::cmp::min(from_block + 1000, latest_block);

    debug!(
        from = from_block,
        to = to_block,
        "Querying events from block range"
    );

    // 创建事件过滤器
    let event_signature = StakeEvent::signature();
    let filter = Filter::new()
        .address(contract_address)
        .from_block(from_block)
        .to_block(to_block)
        .topic0(event_signature);

    // 查询日志
    let logs = provider
        .get_logs(&filter)
        .await
        .map_err(|e| anyhow!("Failed to get logs: {}", e))?;

    debug!(count = logs.len(), "Found events");

    // 处理每个日志
    for log in logs {
        match parse_stake_event(&log) {
            Ok(event) => {
                info!(
                    nonce = event.nonce,
                    amount = event.amount,
                    receiver = event.receiver_address,
                    "Processing StakeEvent"
                );

                // 转换为 StakeEventData
                let event_data = StakeEventData {
                    source_contract: hex::encode(event.source_contract),
                    target_contract: hex::encode(event.target_contract),
                    source_chain_id: event.chain_id,
                    target_chain_id: config.target_chain.chain_id,
                    block_height: event.block_height,
                    amount: event.amount,
                    sender: format!("{:?}", event.sender),  // EVM 发起者地址
                    receiver_address: event.receiver_address.clone(),
                    nonce: event.nonce,
                };

                // 保存到队列文件
                if let Err(e) = save_to_queue(&event_data, &config.queue.path) {
                    error!(nonce = event.nonce, error = %e, "Failed to save event to queue");
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse StakeEvent");
            }
        }
    }

    Ok(to_block)
}

/// 解析 StakeEvent
fn parse_stake_event(log: &Log) -> Result<StakeEvent> {
    // 检查日志是否有足够的 topics
    if log.topics.len() < 3 {
        return Err(anyhow!("Insufficient topics in log"));
    }

    // 解析 indexed 字段
    let source_contract: [u8; 32] = log.topics[1].into();
    let target_contract: [u8; 32] = log.topics[2].into();

    // 解析非 indexed 字段
    let data_tokens = ethers::abi::decode(
        &[
            ParamType::Uint(64),  // chain_id
            ParamType::Uint(64),  // block_height
            ParamType::Uint(64),  // amount
            ParamType::Address,   // sender
            ParamType::String,    // receiver_address
            ParamType::Uint(64),  // nonce
        ],
        &log.data,
    )
    .map_err(|e| anyhow!("Failed to decode log data: {}", e))?;

    let chain_id = data_tokens[0]
        .clone()
        .into_uint()
        .ok_or_else(|| anyhow!("Invalid chain_id"))?
        .as_u64();
    let block_height = data_tokens[1]
        .clone()
        .into_uint()
        .ok_or_else(|| anyhow!("Invalid block_height"))?
        .as_u64();
    let amount = data_tokens[2]
        .clone()
        .into_uint()
        .ok_or_else(|| anyhow!("Invalid amount"))?
        .as_u64();
    let sender = data_tokens[3]
        .clone()
        .into_address()
        .ok_or_else(|| anyhow!("Invalid sender"))?;
    let receiver_address = data_tokens[4]
        .clone()
        .into_string()
        .ok_or_else(|| anyhow!("Invalid receiver_address"))?;
    let nonce = data_tokens[5]
        .clone()
        .into_uint()
        .ok_or_else(|| anyhow!("Invalid nonce"))?
        .as_u64();

    Ok(StakeEvent {
        source_contract,
        target_contract,
        chain_id,
        block_height,
        amount,
        sender,
        receiver_address,
        nonce,
    })
}

/// 保存事件到队列文件
fn save_to_queue(event: &StakeEventData, queue_dir: &Path) -> Result<()> {
    let queue_file = queue_dir.join(format!("event_{}.json", event.nonce));
    let json = serde_json::to_string_pretty(event)?;
    std::fs::write(&queue_file, json)?;
    info!(nonce = event.nonce, path = %queue_file.display(), "Event saved to queue");
    Ok(())
}

#[derive(SerdeSerialize, SerdeDeserialize)]
struct Checkpoint {
    last_block: u64,
    updated_at: String,
}

fn load_checkpoint(path: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    let cp: Checkpoint = serde_json::from_str(&content).ok()?;
    Some(cp.last_block)
}

fn save_checkpoint(path: &Path, block: u64) -> Result<()> {
    let cp = Checkpoint {
        last_block: block,
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    let json = serde_json::to_string_pretty(&cp)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

