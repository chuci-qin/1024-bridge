use anyhow::{anyhow, Result};
use bridge1024_core::config::ChainConfig;
use bridge1024_core::crypto;
use bridge1024_core::types::{BridgeEvent, QueuedEvent};
use ethers::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// 0.01 ETH in wei — warn if relayer balance drops below this.
const MIN_ETH_BALANCE_WEI: u64 = 10_000_000_000_000_000;

type EvmClient = Arc<SignerMiddleware<Provider<Http>, LocalWallet>>;

pub async fn run(
    config: &ChainConfig,
    queue_dir: &str,
    dead_letter_dir: &str,
    bridge_id: &str,
) -> Result<()> {
    let private_key = std::env::var("RELAYER_PRIVATE_KEY").expect("RELAYER_PRIVATE_KEY required");
    let contract_address: Address = std::env::var("CONTRACT_ADDRESS")
        .expect("CONTRACT_ADDRESS required for EVM submitter")
        .parse()
        .map_err(|_| anyhow!("Invalid CONTRACT_ADDRESS"))?;

    let provider = Provider::<Http>::try_from(&config.rpc_url)?;
    let chain_id = config.chain_id;

    let wallet: LocalWallet = private_key
        .trim()
        .trim_start_matches("0x")
        .parse::<LocalWallet>()?
        .with_chain_id(chain_id);

    let client: EvmClient = Arc::new(SignerMiddleware::new(provider, wallet));

    let poll_interval = Duration::from_secs(2);
    let max_retries: u32 = std::env::var("MAX_RETRIES")
        .unwrap_or_else(|_| "10".to_string())
        .parse()?;
    let gas_limit: u64 = std::env::var("GAS_LIMIT")
        .unwrap_or_else(|_| "300000".to_string())
        .parse()?;

    info!(
        relayer = %client.address(),
        contract = %contract_address,
        chain_id = chain_id,
        bridge_id = bridge_id,
        max_retries = max_retries,
        gas_limit = gas_limit,
        "EVM submitter initialized"
    );

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutdown signal received");
                break;
            }
            _ = tokio::time::sleep(poll_interval) => {
                check_balance(&client).await;
                process_queue(
                    &client, contract_address, queue_dir,
                    dead_letter_dir, max_retries, gas_limit,
                ).await;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Balance monitoring
// ---------------------------------------------------------------------------

async fn check_balance(client: &EvmClient) {
    match client.get_balance(client.address(), None).await {
        Ok(balance) => {
            if balance < U256::from(MIN_ETH_BALANCE_WEI) {
                warn!(
                    balance_eth = %ethers::utils::format_ether(balance),
                    "Low relayer ETH balance"
                );
            }
        }
        Err(e) => warn!(error = %e, "Failed to check ETH balance"),
    }
}

// ---------------------------------------------------------------------------
// Queue processing
// ---------------------------------------------------------------------------

async fn process_queue(
    client: &EvmClient,
    contract_address: Address,
    queue_dir: &str,
    dead_letter_dir: &str,
    max_retries: u32,
    gas_limit: u64,
) {
    let entries = match std::fs::read_dir(queue_dir) {
        Ok(e) => e,
        Err(e) => {
            error!(error = %e, "Failed to read queue directory");
            return;
        }
    };

    let mut events: Vec<(PathBuf, QueuedEvent)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<QueuedEvent>(&content) {
                Ok(queued) => events.push((path, queued)),
                Err(e) => warn!(path = %path.display(), error = %e, "Failed to parse event file"),
            },
            Err(e) => warn!(path = %path.display(), error = %e, "Failed to read event file"),
        }
    }

    events.sort_by_key(|(_, q)| q.event.nonce);

    for (path, _) in &events {
        if let Err(e) = process_event(
            client,
            contract_address,
            path,
            dead_letter_dir,
            max_retries,
            gas_limit,
        )
        .await
        {
            error!(path = %path.display(), error = %e, "Failed to process event");
        }
    }
}

// ---------------------------------------------------------------------------
// Single-event processing with file locking (REL-H6)
// ---------------------------------------------------------------------------

async fn process_event(
    client: &EvmClient,
    contract_address: Address,
    path: &Path,
    dead_letter_dir: &str,
    max_retries: u32,
    gas_limit: u64,
) -> Result<()> {
    let processing_path = PathBuf::from(format!("{}.processing", path.display()));
    std::fs::rename(path, &processing_path)
        .map_err(|e| anyhow!("Failed to acquire file lock (rename to .processing): {}", e))?;

    let content = std::fs::read_to_string(&processing_path)?;
    let mut queued: QueuedEvent = serde_json::from_str(&content)?;

    if queued.retries >= max_retries {
        info!(
            nonce = queued.event.nonce,
            retries = queued.retries,
            "Retry limit reached, moving to dead-letter queue"
        );
        move_to_dead_letter(&processing_path, dead_letter_dir);
        return Ok(());
    }

    info!(
        nonce = queued.event.nonce,
        retries = queued.retries,
        amount = queued.event.amount,
        "Processing event"
    );

    // ECDSA signing: SHA-256 hash of canonical JSON → EIP-191 personal-sign
    let message_hash = crypto::hash_event_data_json(&queued.event);
    let wallet = client.signer();
    let signature = crypto::sign_ecdsa_eip191(&message_hash, wallet)
        .await
        .map_err(|e| anyhow!("ECDSA signing failed: {}", e))?;

    match submit_to_evm(
        client,
        contract_address,
        &queued.event,
        &signature,
        gas_limit,
    )
    .await
    {
        Ok(tx_hash) => {
            info!(
                nonce = queued.event.nonce,
                tx_hash = %tx_hash,
                "Transaction confirmed, removing from queue"
            );
            let _ = std::fs::remove_file(&processing_path);
            Ok(())
        }
        Err(e) => {
            error!(nonce = queued.event.nonce, error = %e, "EVM submission failed");

            queued.retries += 1;
            queued.last_retry_at = Some(now_secs());

            if queued.retries >= max_retries {
                warn!(
                    nonce = queued.event.nonce,
                    "Exhausted all retries, moving to dead-letter queue"
                );
                let updated = serde_json::to_string_pretty(&queued)?;
                std::fs::write(&processing_path, updated)?;
                move_to_dead_letter(&processing_path, dead_letter_dir);
            } else {
                // Exponential backoff (REL-C4): 2^retry seconds, capped at 64 s
                let delay = Duration::from_secs(2u64.saturating_pow(queued.retries.min(6)));
                info!(
                    nonce = queued.event.nonce,
                    next_retry = queued.retries,
                    delay_secs = delay.as_secs(),
                    "Backing off before next retry"
                );
                tokio::time::sleep(delay).await;

                let updated = serde_json::to_string_pretty(&queued)?;
                std::fs::write(&processing_path, &updated)?;
                std::fs::rename(&processing_path, path)?;
            }
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// EVM transaction submission with EIP-1559 (REL-M4) + explicit gas (REL-M3)
// ---------------------------------------------------------------------------

async fn submit_to_evm(
    client: &EvmClient,
    contract_address: Address,
    event: &BridgeEvent,
    signature: &[u8],
    gas_limit: u64,
) -> Result<String> {
    let call_data = encode_submit_signature(event, signature)?;

    let (max_fee, max_priority_fee) = client
        .provider()
        .estimate_eip1559_fees(None)
        .await
        .unwrap_or((
            U256::from(50_000_000_000u64), // 50 gwei fallback
            U256::from(2_000_000_000u64),  // 2 gwei priority fallback
        ));

    let tx = Eip1559TransactionRequest::new()
        .to(contract_address)
        .data(call_data)
        .max_fee_per_gas(max_fee)
        .max_priority_fee_per_gas(max_priority_fee)
        .gas(gas_limit);

    info!(
        max_fee_gwei = %ethers::utils::format_units(max_fee, "gwei").unwrap_or_default(),
        priority_gwei = %ethers::utils::format_units(max_priority_fee, "gwei").unwrap_or_default(),
        gas_limit = gas_limit,
        "Sending EIP-1559 transaction"
    );

    let pending_tx = client
        .send_transaction(tx, None)
        .await
        .map_err(|e| anyhow!("Failed to send transaction: {}", e))?;

    let receipt = pending_tx
        .await?
        .ok_or_else(|| anyhow!("Transaction dropped from mempool"))?;

    Ok(format!("{:?}", receipt.transaction_hash))
}

// ---------------------------------------------------------------------------
// ABI encoding: submitSignature((bytes32,...), bytes)
// ---------------------------------------------------------------------------

fn encode_submit_signature(event: &BridgeEvent, signature: &[u8]) -> Result<Bytes> {
    use ethers::abi::{encode, Token};

    let fn_sig = "submitSignature((bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,string,uint64),bytes)";
    let selector = &ethers::utils::keccak256(fn_sig.as_bytes())[0..4];

    let source_contract = parse_bytes32(&event.source_contract)?;
    let target_contract = parse_bytes32(&event.target_contract)?;
    let sender_bytes = parse_bytes32(&event.sender)?;

    let event_tuple = Token::Tuple(vec![
        Token::FixedBytes(source_contract.to_vec()),
        Token::FixedBytes(target_contract.to_vec()),
        Token::Uint(U256::from(event.source_chain_id)),
        Token::Uint(U256::from(event.target_chain_id)),
        Token::Uint(U256::from(event.block_height)),
        Token::Uint(U256::from(event.amount)),
        Token::FixedBytes(sender_bytes.to_vec()),
        Token::String(event.receiver_address.clone()),
        Token::Uint(U256::from(event.nonce)),
    ]);

    let sig_token = Token::Bytes(signature.to_vec());
    let encoded = encode(&[event_tuple, sig_token]);

    let mut data = Vec::with_capacity(4 + encoded.len());
    data.extend_from_slice(selector);
    data.extend_from_slice(&encoded);

    Ok(Bytes::from(data))
}

/// Parse a hex string (with or without 0x) or base58 pubkey into a 32-byte
/// array, left-padding shorter values with zeroes.
fn parse_bytes32(s: &str) -> Result<[u8; 32]> {
    let hex_str = s.strip_prefix("0x").unwrap_or(s);
    if hex_str.chars().all(|c| c.is_ascii_hexdigit()) && !hex_str.is_empty() {
        let bytes = hex::decode(hex_str)?;
        let mut result = [0u8; 32];
        let start = 32usize.saturating_sub(bytes.len());
        let len = bytes.len().min(32);
        result[start..start + len].copy_from_slice(&bytes[..len]);
        return Ok(result);
    }
    if let Ok(bytes) = bs58::decode(s).into_vec() {
        if bytes.len() == 32 {
            let mut result = [0u8; 32];
            result.copy_from_slice(&bytes);
            return Ok(result);
        }
    }
    Err(anyhow!("Cannot parse as bytes32: {}", s))
}

// ---------------------------------------------------------------------------
// Dead-letter queue
// ---------------------------------------------------------------------------

fn move_to_dead_letter(processing_path: &Path, dead_letter_dir: &str) {
    let filename = processing_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown.json.processing");
    let original_name = filename.strip_suffix(".processing").unwrap_or(filename);
    let dest = Path::new(dead_letter_dir).join(original_name);

    if let Err(e) = std::fs::rename(processing_path, &dest) {
        error!(error = %e, "Failed to move to dead-letter (rename), trying copy+delete");
        if let Ok(content) = std::fs::read(processing_path) {
            let _ = std::fs::write(&dest, content);
            let _ = std::fs::remove_file(processing_path);
        }
    } else {
        info!(dest = %dest.display(), "Moved event to dead-letter queue");
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
