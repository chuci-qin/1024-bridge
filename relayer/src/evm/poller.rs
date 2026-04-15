use anyhow::{Context, Result};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, BlockNumber, Filter, Log, H256};
use tracing::{debug, warn};

use crate::types::StakeEventData;

/// keccak256("StakeEvent(bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64)")
fn stake_event_topic() -> H256 {
    let sig = "StakeEvent(bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64)";
    H256::from(ethers::utils::keccak256(sig.as_bytes()))
}

/// Get the finalized block number from the chain. Falls back to `latest` if
/// the RPC does not support the `finalized` tag (some testnets).
async fn get_finalized_block_number(provider: &Provider<Http>) -> Result<u64> {
    match provider.get_block(BlockNumber::Finalized).await {
        Ok(Some(block)) => Ok(block
            .number
            .context("finalized block missing number field")?
            .as_u64()),
        Ok(None) | Err(_) => {
            warn!("RPC does not support 'finalized' block tag, falling back to 'latest'");
            Ok(provider
                .get_block_number()
                .await
                .context("get latest block")?
                .as_u64())
        }
    }
}

/// Parse a StakeEvent from an EVM log entry.
///
/// Layout:
/// - topic\[1\]: sourceContract (indexed bytes32)
/// - topic\[2\]: targetContract (indexed bytes32)
/// - data: ABI-encoded (uint64, uint64, uint64, uint64, bytes32, bytes32, uint64)
pub fn parse_stake_event(log: &Log) -> Result<StakeEventData> {
    if log.topics.len() < 3 {
        anyhow::bail!("StakeEvent requires at least 3 topics");
    }

    let source_contract: [u8; 32] = log.topics[1].into();
    let target_contract: [u8; 32] = log.topics[2].into();

    let data = &log.data.0;
    if data.len() < 7 * 32 {
        anyhow::bail!(
            "StakeEvent data too short: {} bytes, expected >= {}",
            data.len(),
            7 * 32
        );
    }

    fn read_u64(slice: &[u8], offset: usize) -> u64 {
        let word = &slice[offset..offset + 32];
        u64::from_be_bytes(word[24..32].try_into().unwrap())
    }

    fn read_bytes32(slice: &[u8], offset: usize) -> [u8; 32] {
        let mut out = [0u8; 32];
        out.copy_from_slice(&slice[offset..offset + 32]);
        out
    }

    let source_chain_id = read_u64(data, 0);
    let target_chain_id = read_u64(data, 32);
    let block_height = read_u64(data, 64);
    let amount = read_u64(data, 96);
    let sender = read_bytes32(data, 128);
    let receiver = read_bytes32(data, 160);
    let nonce = read_u64(data, 192);

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

/// Poll EVM chain for StakeEvent logs starting from the checkpoint.
///
/// Only reads up to the chain's **finalized** block to avoid picking up events
/// from blocks that could still be reorged.
pub async fn poll_evm_events(
    provider: &Provider<Http>,
    contract_address: Address,
    from_block: u64,
    max_block_range: u64,
) -> Result<(Vec<StakeEventData>, u64)> {
    let finalized = get_finalized_block_number(provider).await?;

    if from_block > finalized {
        return Ok((vec![], from_block));
    }

    let to_block = std::cmp::min(from_block + max_block_range, finalized);

    let filter = Filter::new()
        .address(contract_address)
        .topic0(stake_event_topic())
        .from_block(from_block)
        .to_block(to_block);

    let logs = provider.get_logs(&filter).await.context("get_logs")?;

    let mut events = Vec::new();
    for log in &logs {
        match parse_stake_event(log) {
            Ok(event) => {
                debug!(
                    nonce = event.nonce,
                    amount = event.amount,
                    block = ?log.block_number,
                    "Parsed EVM StakeEvent"
                );
                events.push(event);
            }
            Err(e) => {
                warn!(tx = ?log.transaction_hash, "Failed to parse StakeEvent: {e}");
            }
        }
    }

    Ok((events, to_block + 1))
}

/// Determine the starting block when no checkpoint exists.
/// Scans back `scan_back` blocks from the current finalized head.
pub async fn initial_from_block(provider: &Provider<Http>, scan_back: u64) -> Result<u64> {
    let finalized = get_finalized_block_number(provider).await?;
    Ok(finalized.saturating_sub(scan_back))
}
