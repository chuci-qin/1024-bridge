use anyhow::{Context, Result};
use base64::Engine;
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_transaction_status::UiTransactionEncoding;
use tracing::{debug, warn};

use crate::types::StakeEventData;

/// Anchor event discriminator: SHA-256("event:StakeEvent")[..8]
fn stake_event_discriminator() -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update("event:StakeEvent");
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

/// Parse a StakeEvent from Anchor program log data (8-byte disc + Borsh fields).
fn parse_stake_event_from_data(data: &[u8]) -> Result<StakeEventData> {
    let disc = stake_event_discriminator();
    if data.len() < 8 + StakeEventData::BORSH_LEN {
        anyhow::bail!("StakeEvent data too short: {} bytes", data.len());
    }
    if data[..8] != disc {
        anyhow::bail!("Not a StakeEvent (discriminator mismatch)");
    }

    let body = &data[8..];
    let mut offset = 0;

    let mut source_contract = [0u8; 32];
    source_contract.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    let mut target_contract = [0u8; 32];
    target_contract.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    let source_chain_id = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;
    let target_chain_id = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;
    let block_height = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;
    let amount = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;

    let mut sender = [0u8; 32];
    sender.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    let mut receiver = [0u8; 32];
    receiver.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    let nonce = u64::from_le_bytes(body[offset..offset + 8].try_into()?);

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

/// Extract StakeEvents from a transaction's log messages.
fn extract_events_from_logs(logs: &[String]) -> Vec<StakeEventData> {
    let b64_engine = base64::engine::general_purpose::STANDARD;
    let mut events = Vec::new();

    for log_line in logs {
        if let Some(data_str) = log_line.strip_prefix("Program data: ") {
            if let Ok(data) = b64_engine.decode(data_str.trim()) {
                if let Ok(event) = parse_stake_event_from_data(&data) {
                    events.push(event);
                }
            }
        }
    }

    events
}

/// Poll SVM chain for StakeEvent transactions with automatic pagination.
///
/// Fetches signatures in pages of `batch_size`, paginating via the `before`
/// cursor, up to `max_total` signatures. This avoids overwhelming public RPCs
/// while still scanning enough history to not miss events.
///
/// `until_sig`: if Some, stop scanning at this signature (exclusive).
/// Returns (events oldest-first, newest_signature).
pub fn poll_svm_events(
    rpc: &RpcClient,
    program_id: &Pubkey,
    until_sig: Option<&Signature>,
    batch_size: usize,
    max_total: usize,
) -> Result<(Vec<StakeEventData>, Option<Signature>)> {
    use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;

    let mut all_sig_infos = Vec::new();
    let mut before_cursor: Option<Signature> = None;

    loop {
        let config = GetConfirmedSignaturesForAddress2Config {
            before: before_cursor,
            until: until_sig.copied(),
            limit: Some(batch_size),
            commitment: Some(CommitmentConfig::finalized()),
        };

        let batch = rpc
            .get_signatures_for_address_with_config(program_id, config)
            .context("getSignaturesForAddress")?;

        let batch_len = batch.len();

        if batch.is_empty() {
            break;
        }

        let oldest_sig: Signature = batch
            .last()
            .unwrap()
            .signature
            .parse()
            .context("parse oldest sig in page")?;

        all_sig_infos.extend(batch);

        if all_sig_infos.len() >= max_total {
            all_sig_infos.truncate(max_total);
            break;
        }

        if batch_len < batch_size {
            break;
        }

        before_cursor = Some(oldest_sig);

        debug!(
            fetched = all_sig_infos.len(),
            max_total,
            "Paginating getSignaturesForAddress"
        );
    }

    if all_sig_infos.is_empty() {
        return Ok((vec![], None));
    }

    let newest_sig = all_sig_infos[0]
        .signature
        .parse::<Signature>()
        .context("parse newest signature")?;

    debug!(
        total_sigs = all_sig_infos.len(),
        "Fetching transactions for signatures"
    );

    let mut all_events = Vec::new();

    for sig_info in all_sig_infos.iter().rev() {
        if sig_info.err.is_some() {
            continue;
        }

        let sig: Signature = sig_info
            .signature
            .parse()
            .context("parse tx signature")?;

        let tx_config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::Json),
            commitment: Some(CommitmentConfig::finalized()),
            max_supported_transaction_version: Some(0),
        };

        match rpc.get_transaction_with_config(&sig, tx_config) {
            Ok(tx_response) => {
                if let Some(meta) = tx_response.transaction.meta {
                    let log_msgs: Option<&Vec<String>> = meta.log_messages.as_ref().into();
                    if let Some(logs) = log_msgs {
                        let events = extract_events_from_logs(logs);
                        for event in events {
                            debug!(
                                nonce = event.nonce,
                                amount = event.amount,
                                tx = %sig,
                                "Parsed SVM StakeEvent"
                            );
                            all_events.push(event);
                        }
                    }
                }
            }
            Err(e) => {
                warn!(tx = %sig, "Failed to fetch transaction: {e}");
            }
        }
    }

    Ok((all_events, Some(newest_sig)))
}
