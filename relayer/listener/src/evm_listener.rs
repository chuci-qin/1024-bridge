use anyhow::{anyhow, Result};
use bridge1024_core::config::ChainConfig;
use bridge1024_core::types::{BridgeEvent, QueuedEvent};
use ethers::abi::ParamType;
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, Filter, Log, H256};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    last_block: u64,
    last_block_hash: Option<String>,
}

pub async fn run(
    config: &ChainConfig,
    target_chain_id: u64,
    queue_dir: &str,
    checkpoint_path: &str,
    bridge_id: &str,
) -> Result<()> {
    let provider = Provider::<Http>::try_from(&config.rpc_url)
        .map_err(|e| anyhow!("Failed to create EVM provider: {}", e))?;
    let provider = Arc::new(provider);

    let contract_addr_str = config
        .contract_address
        .as_deref()
        .unwrap_or(&config.token_address);
    let contract_address: Address = contract_addr_str
        .parse()
        .map_err(|e| anyhow!("Invalid contract address '{}': {}", contract_addr_str, e))?;

    let confirmation_blocks = config.confirmation_blocks.unwrap_or(12);
    let poll_interval = std::time::Duration::from_secs(5);

    let mut checkpoint = load_checkpoint(checkpoint_path).unwrap_or(Checkpoint {
        last_block: 0,
        last_block_hash: None,
    });

    if checkpoint.last_block == 0 {
        let current = provider.get_block_number().await?.as_u64();
        checkpoint.last_block = current;
        save_checkpoint(checkpoint_path, &checkpoint)?;
        info!(block = current, "No checkpoint found, starting from current block");
    }

    // StakeEvent(bytes32 indexed, bytes32 indexed, uint64, uint64, uint64, address, string, uint64)
    let stake_event_sig = H256::from(ethers::utils::keccak256(
        b"StakeEvent(bytes32,bytes32,uint64,uint64,uint64,address,string,uint64)",
    ));

    info!(
        contract = %contract_address,
        from_block = checkpoint.last_block,
        confirmations = confirmation_blocks,
        "EVM listener initialized"
    );

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutdown signal received, persisting checkpoint");
                save_checkpoint(checkpoint_path, &checkpoint)?;
                break;
            }
            _ = tokio::time::sleep(poll_interval) => {
                match poll_events(
                    &provider,
                    contract_address,
                    &stake_event_sig,
                    &mut checkpoint,
                    confirmation_blocks,
                    target_chain_id,
                    queue_dir,
                    checkpoint_path,
                    bridge_id,
                ).await {
                    Ok(count) => {
                        if count > 0 {
                            info!(events = count, block = checkpoint.last_block, "Processed EVM events");
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Error polling EVM events");
                    }
                }
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn poll_events(
    provider: &Provider<Http>,
    contract_address: Address,
    event_sig: &H256,
    checkpoint: &mut Checkpoint,
    confirmation_blocks: u64,
    target_chain_id: u64,
    queue_dir: &str,
    checkpoint_path: &str,
    bridge_id: &str,
) -> Result<usize> {
    let latest_block = provider.get_block_number().await?.as_u64();
    let safe_head = latest_block.saturating_sub(confirmation_blocks);

    if safe_head <= checkpoint.last_block {
        return Ok(0);
    }

    // Reorg detection: verify the block hash at our last checkpoint
    if let Some(ref expected_hash) = checkpoint.last_block_hash {
        if let Some(block) = provider.get_block(checkpoint.last_block).await? {
            if let Some(hash) = block.hash {
                let actual_hash = format!("{:?}", hash);
                if &actual_hash != expected_hash {
                    let rollback = checkpoint.last_block.saturating_sub(confirmation_blocks);
                    warn!(
                        expected_hash = %expected_hash,
                        actual_hash = %actual_hash,
                        old_block = checkpoint.last_block,
                        rollback_to = rollback,
                        "Reorg detected, rolling back"
                    );
                    checkpoint.last_block = rollback;
                    checkpoint.last_block_hash = None;
                    save_checkpoint(checkpoint_path, checkpoint)?;
                    return Ok(0);
                }
            }
        }
    }

    let from_block = checkpoint.last_block + 1;
    let to_block = std::cmp::min(from_block + 99, safe_head);

    debug!(from = from_block, to = to_block, latest = latest_block, "Querying block range");

    let filter = Filter::new()
        .address(contract_address)
        .from_block(from_block)
        .to_block(to_block)
        .topic0(*event_sig);

    let logs = match provider.get_logs(&filter).await {
        Ok(logs) => logs,
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("beyond") || err_str.contains("ahead") {
                warn!(from = from_block, to = to_block, "RPC block range inconsistency, will retry");
                return Ok(0);
            }
            return Err(anyhow!("Failed to get logs: {}", e));
        }
    };

    debug!(count = logs.len(), "Received logs");

    let mut count = 0;
    for log in &logs {
        match parse_stake_event(log, target_chain_id) {
            Ok(event) => {
                let tx_hash = log
                    .transaction_hash
                    .map(|h| format!("{:?}", h))
                    .unwrap_or_default();

                info!(
                    nonce = event.nonce,
                    amount = event.amount,
                    sender = %event.sender,
                    receiver = %event.receiver_address,
                    tx = %tx_hash,
                    "Captured StakeEvent"
                );

                let now = now_epoch();
                let queued = QueuedEvent {
                    bridge_id: bridge_id.to_string(),
                    event,
                    retries: 0,
                    max_retries: 10,
                    created_at: now,
                    last_retry_at: None,
                    source_tx_hash: Some(tx_hash),
                    detected_at: now,
                };

                write_event_to_queue(queue_dir, &queued)?;
                count += 1;
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse StakeEvent from log");
            }
        }
    }

    // Record the block hash at to_block for future reorg detection
    if let Some(block) = provider.get_block(to_block).await? {
        checkpoint.last_block_hash = block.hash.map(|h| format!("{:?}", h));
    }
    checkpoint.last_block = to_block;
    save_checkpoint(checkpoint_path, checkpoint)?;

    Ok(count)
}

fn parse_stake_event(log: &Log, target_chain_id: u64) -> Result<BridgeEvent> {
    if log.topics.len() < 3 {
        return Err(anyhow!(
            "Insufficient topics: expected >= 3, got {}",
            log.topics.len()
        ));
    }

    let source_contract: [u8; 32] = log.topics[1].into();
    let target_contract: [u8; 32] = log.topics[2].into();

    let data_tokens = ethers::abi::decode(
        &[
            ParamType::Uint(64), // chain_id
            ParamType::Uint(64), // block_height
            ParamType::Uint(64), // amount
            ParamType::Address,  // sender
            ParamType::String,   // receiver_address
            ParamType::Uint(64), // nonce
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
        .ok_or_else(|| anyhow!("Invalid sender address"))?;
    let receiver_address = data_tokens[4]
        .clone()
        .into_string()
        .ok_or_else(|| anyhow!("Invalid receiver_address"))?;
    let nonce = data_tokens[5]
        .clone()
        .into_uint()
        .ok_or_else(|| anyhow!("Invalid nonce"))?
        .as_u64();

    Ok(BridgeEvent {
        source_contract: hex::encode(source_contract),
        target_contract: hex::encode(target_contract),
        source_chain_id: chain_id,
        target_chain_id,
        block_height,
        amount,
        sender: format!("{sender:?}"),
        receiver_address,
        nonce,
    })
}

/// Atomic write: write to .tmp then rename to avoid partial reads.
fn write_event_to_queue(queue_dir: &str, event: &QueuedEvent) -> Result<()> {
    let timestamp = now_epoch();
    let filename = format!("event_{}_{}.json", event.event.nonce, timestamp);
    let path = std::path::Path::new(queue_dir).join(&filename);
    let tmp_path = path.with_extension("json.tmp");

    let json = serde_json::to_string_pretty(event)?;
    std::fs::write(&tmp_path, &json)?;
    std::fs::rename(&tmp_path, &path)?;

    info!(nonce = event.event.nonce, path = %path.display(), "Event written to queue");
    Ok(())
}

fn load_checkpoint(path: &str) -> Option<Checkpoint> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Atomic checkpoint save: write to .tmp then rename.
fn save_checkpoint(path: &str, checkpoint: &Checkpoint) -> Result<()> {
    let tmp_path = format!("{}.tmp", path);
    let json = serde_json::to_string_pretty(checkpoint)?;
    std::fs::write(&tmp_path, &json)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
