use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use borsh::BorshDeserialize;
use bridge1024_core::config::ChainConfig;
use bridge1024_core::types::{BridgeEvent, QueuedEvent};
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature as SolSignature;
use solana_transaction_status::UiTransactionEncoding;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    last_signature: Option<String>,
    last_slot: u64,
}

pub async fn run(
    config: &ChainConfig,
    target_chain_id: u64,
    queue_dir: &str,
    checkpoint_path: &str,
    bridge_id: &str,
) -> Result<()> {
    let commitment = CommitmentConfig::finalized();
    let client = RpcClient::new_with_commitment(config.rpc_url.clone(), commitment);
    let poll_interval = std::time::Duration::from_secs(3);

    let program_id = Pubkey::from_str(&config.contract_address)
        .map_err(|e| anyhow!("Invalid program address '{}': {}", config.contract_address, e))?;

    let mut checkpoint = load_checkpoint(checkpoint_path).unwrap_or(Checkpoint {
        last_signature: None,
        last_slot: 0,
    });

    info!(
        program = %program_id,
        last_signature = ?checkpoint.last_signature,
        last_slot = checkpoint.last_slot,
        "SVM listener initialized"
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
                    &client,
                    &program_id,
                    target_chain_id,
                    &mut checkpoint,
                    queue_dir,
                    checkpoint_path,
                    bridge_id,
                ).await {
                    Ok(count) => {
                        if count > 0 {
                            info!(events = count, slot = checkpoint.last_slot, "Processed SVM events");
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Error polling SVM events");
                    }
                }
            }
        }
    }

    Ok(())
}

async fn poll_events(
    client: &RpcClient,
    program_id: &Pubkey,
    target_chain_id: u64,
    checkpoint: &mut Checkpoint,
    queue_dir: &str,
    checkpoint_path: &str,
    bridge_id: &str,
) -> Result<usize> {
    let until = checkpoint
        .last_signature
        .as_ref()
        .and_then(|s| SolSignature::from_str(s).ok());

    let sig_config = GetConfirmedSignaturesForAddress2Config {
        before: None,
        until,
        limit: Some(100),
        commitment: Some(CommitmentConfig::finalized()),
    };

    let signatures = client
        .get_signatures_for_address_with_config(program_id, sig_config)
        .await
        .map_err(|e| anyhow!("Failed to fetch signatures: {}", e))?;

    if signatures.is_empty() {
        return Ok(0);
    }

    debug!(count = signatures.len(), "Fetched new signatures");

    // Results arrive newest-first; reverse for chronological processing
    let mut sorted = signatures;
    sorted.reverse();

    let mut count = 0;

    for sig_info in &sorted {
        if sig_info.err.is_some() {
            debug!(sig = %sig_info.signature, "Skipping failed transaction");
            continue;
        }

        let signature = SolSignature::from_str(&sig_info.signature)
            .map_err(|e| anyhow!("Invalid signature '{}': {}", sig_info.signature, e))?;

        let tx_config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::Json),
            commitment: Some(CommitmentConfig::finalized()),
            max_supported_transaction_version: Some(0),
        };

        let tx = match client
            .get_transaction_with_config(&signature, tx_config)
            .await
        {
            Ok(tx) => tx,
            Err(e) => {
                warn!(sig = %sig_info.signature, error = %e, "Failed to fetch transaction, skipping");
                continue;
            }
        };

        // REL-C1 fix: extract the actual signer from the transaction
        let tx_json = serde_json::to_value(&tx.transaction.transaction)?;
        let signer = extract_signer_from_json(&tx_json);

        // Extract log messages from transaction meta
        let logs = if let Some(ref meta) = tx.transaction.meta {
            let meta_json = serde_json::to_value(meta)?;
            extract_logs_from_json(&meta_json)
        } else {
            vec![]
        };

        for log_line in &logs {
            if !log_line.contains("Program data:") {
                continue;
            }

            if let Some(mut event) = parse_anchor_event(log_line, target_chain_id) {
                // Set sender to actual transaction signer instead of receiver_address
                if let Some(ref signer_pubkey) = signer {
                    event.sender = signer_pubkey.clone();
                }

                info!(
                    nonce = event.nonce,
                    amount = event.amount,
                    sender = %event.sender,
                    receiver = %event.receiver_address,
                    sig = %sig_info.signature,
                    "Captured StakeEvent"
                );

                let queued = QueuedEvent {
                    bridge_id: bridge_id.to_string(),
                    event,
                    source_tx_hash: Some(sig_info.signature.clone()),
                    detected_at: now_epoch(),
                };

                write_event_to_queue(queue_dir, &queued)?;
                count += 1;
            }
        }

        checkpoint.last_signature = Some(sig_info.signature.clone());
        checkpoint.last_slot = sig_info.slot;
    }

    if count > 0 || !sorted.is_empty() {
        save_checkpoint(checkpoint_path, checkpoint)?;
    }

    Ok(count)
}

/// Extract the fee-payer/signer pubkey from a JSON-encoded transaction.
/// The first account key is the fee payer, which is the transaction signer.
fn extract_signer_from_json(tx_json: &serde_json::Value) -> Option<String> {
    tx_json
        .get("message")
        .and_then(|m| m.get("accountKeys"))
        .and_then(|keys| keys.as_array())
        .and_then(|keys| keys.first())
        .and_then(|k| {
            // Raw message: plain string key
            // Parsed message: { "pubkey": "...", "signer": true, ... }
            k.as_str()
                .map(String::from)
                .or_else(|| k.get("pubkey").and_then(|p| p.as_str()).map(String::from))
        })
}

fn extract_logs_from_json(meta_json: &serde_json::Value) -> Vec<String> {
    meta_json
        .get("logMessages")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Anchor event layout emitted by the bridge program.
/// The event is Borsh-serialized after an 8-byte discriminator.
#[derive(BorshDeserialize)]
struct AnchorStakeEvent {
    source_contract: String,
    target_contract: String,
    chain_id: u64,
    block_height: u64,
    amount: u64,
    receiver_address: String,
    nonce: u64,
}

/// Parse an Anchor `Program data:` log line into a BridgeEvent.
/// Anchor logs are base64-encoded with an 8-byte discriminator prefix.
fn parse_anchor_event(log: &str, target_chain_id: u64) -> Option<BridgeEvent> {
    let data_str = log.strip_prefix("Program data: ")?;
    let data = general_purpose::STANDARD.decode(data_str.trim()).ok()?;

    if data.len() <= 8 {
        return None;
    }

    let event_data = &data[8..];
    let anchor_event = AnchorStakeEvent::try_from_slice(event_data).ok()?;

    Some(BridgeEvent {
        source_contract: anchor_event.source_contract,
        target_contract: anchor_event.target_contract,
        source_chain_id: anchor_event.chain_id,
        target_chain_id,
        block_height: anchor_event.block_height,
        amount: anchor_event.amount,
        sender: String::new(), // Set by caller from transaction signer (REL-C1)
        receiver_address: anchor_event.receiver_address,
        nonce: anchor_event.nonce,
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
