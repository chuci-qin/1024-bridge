use crate::config::ListenerConfig;
use anyhow::{anyhow, Result};
use shared::types::StakeEventData;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
    signature::Signature,
};
use solana_transaction_status::UiTransactionEncoding;
use std::{path::Path, str::FromStr};
use tracing::{debug, error, info, warn};

/// Start the Solana event listener.
/// Polls Solana for new transaction signatures against the bridge program,
/// parses StakeEvents from transaction logs, and writes them to the file queue.
pub async fn start_listener(config: ListenerConfig) -> Result<()> {
    info!("Starting Solana event listener");
    info!(
        rpc = config.source_chain.rpc_url,
        program = config.source_chain.contract_address,
        "Connecting to Solana"
    );

    let commitment = config.source_chain.commitment.as_deref().unwrap_or("confirmed");
    let rpc_client = RpcClient::new_with_commitment(
        config.source_chain.rpc_url.clone(),
        CommitmentConfig::from_str(commitment)
            .unwrap_or(CommitmentConfig::confirmed()),
    );

    let program_id = Pubkey::from_str(&config.source_chain.contract_address)
        .map_err(|e| anyhow!("Invalid Solana program ID: {}", e))?;

    let queue_dir = &config.queue.path;
    std::fs::create_dir_all(queue_dir)?;
    info!(queue_path = %queue_dir.display(), "Queue directory initialized");

    info!("Connected to Solana, starting to poll for events");

    let mut last_signature: Option<Signature> = None;

    loop {
        match poll_events(&rpc_client, &program_id, &last_signature, &config).await {
            Ok(new_sig) => {
                if let Some(sig) = new_sig {
                    last_signature = Some(sig);
                }
            }
            Err(e) => {
                error!("Error polling Solana events: {}", e);
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

/// Poll for new transactions on the Solana bridge program
async fn poll_events(
    rpc_client: &RpcClient,
    program_id: &Pubkey,
    last_signature: &Option<Signature>,
    config: &ListenerConfig,
) -> Result<Option<Signature>> {
    use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
    use solana_client::rpc_config::RpcTransactionConfig;
    use solana_transaction_status::option_serializer::OptionSerializer;

    let sig_config = GetConfirmedSignaturesForAddress2Config {
        before: None,
        until: *last_signature,
        limit: Some(50),
        commitment: Some(CommitmentConfig::confirmed()),
    };

    let signatures = rpc_client
        .get_signatures_for_address_with_config(program_id, sig_config)
        .map_err(|e| anyhow!("Failed to get signatures: {}", e))?;

    if signatures.is_empty() {
        return Ok(None);
    }

    debug!(count = signatures.len(), "Found new Solana transactions");

    let mut newest_sig = None;

    // Process signatures in chronological order (oldest first, API returns newest first)
    for sig_info in signatures.iter().rev() {
        if sig_info.err.is_some() {
            continue;
        }

        let signature = Signature::from_str(&sig_info.signature)
            .map_err(|e| anyhow!("Invalid signature: {}", e))?;

        let tx_config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::Json),
            commitment: Some(CommitmentConfig::confirmed()),
            max_supported_transaction_version: Some(0),
        };

        let tx = match rpc_client.get_transaction_with_config(&signature, tx_config) {
            Ok(t) => t,
            Err(e) => {
                warn!(sig = %signature, error = %e, "Failed to get transaction");
                continue;
            }
        };

        // Extract log messages from transaction metadata
        let log_messages = match &tx.transaction.meta {
            Some(meta) => match &meta.log_messages {
                OptionSerializer::Some(logs) => logs.clone(),
                _ => continue,
            },
            None => continue,
        };

        // Parse StakeEvent from Anchor program logs
        if let Some(event_data) = parse_stake_event_from_logs(&log_messages, config) {
            info!(
                nonce = event_data.nonce,
                amount = event_data.amount,
                sender = event_data.sender,
                receiver = event_data.receiver_address,
                "Parsed Solana StakeEvent"
            );

            if let Err(e) = save_to_queue(&event_data, &config.queue.path) {
                error!(nonce = event_data.nonce, error = %e, "Failed to save event to queue");
            }
        }

        newest_sig = Some(signature);
    }

    Ok(newest_sig)
}

/// Parse StakeEvent from Anchor program log lines.
///
/// Anchor events are emitted as base64-encoded data in log lines prefixed with
/// "Program data: ". The event discriminator is a SHA256 hash of "event:StakeEvent".
fn parse_stake_event_from_logs(
    logs: &[String],
    config: &ListenerConfig,
) -> Option<StakeEventData> {
    use anchor_event_parser::parse_anchor_event;

    for log_line in logs {
        if !log_line.starts_with("Program data: ") {
            continue;
        }

        let b64_data = &log_line["Program data: ".len()..];
        if let Some(event) = parse_anchor_event(b64_data, config) {
            return Some(event);
        }
    }
    None
}

mod anchor_event_parser {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Parse an Anchor event from base64-encoded program data.
    pub fn parse_anchor_event(b64_data: &str, config: &ListenerConfig) -> Option<StakeEventData> {
        let data = base64_decode(b64_data)?;

        // Anchor event discriminator: first 8 bytes = sha256("event:StakeEvent")[..8]
        let mut hasher = Sha256::new();
        hasher.update(b"event:StakeEvent");
        let disc = &hasher.finalize()[..8];

        if data.len() < 8 || &data[..8] != disc {
            return None;
        }

        let payload = &data[8..];
        parse_stake_event_payload(payload, config)
    }

    fn base64_decode(s: &str) -> Option<Vec<u8>> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()
    }

    /// Parse StakeEvent fields from Borsh-like Anchor serialization.
    /// Field order (from Anchor IDL): source_contract, target_contract, chain_id,
    /// block_height, amount, sender, receiver_address, nonce
    fn parse_stake_event_payload(data: &[u8], config: &ListenerConfig) -> Option<StakeEventData> {
        let mut offset = 0;

        let source_contract = read_string(data, &mut offset)?;
        let target_contract = read_string(data, &mut offset)?;
        let chain_id = read_u64(data, &mut offset)?;
        let block_height = read_u64(data, &mut offset)?;
        let amount = read_u64(data, &mut offset)?;
        let sender = read_string(data, &mut offset)?;
        let receiver_address = read_string(data, &mut offset)?;
        let nonce = read_u64(data, &mut offset)?;

        Some(StakeEventData {
            source_contract,
            target_contract,
            source_chain_id: chain_id,
            target_chain_id: config.target_chain.chain_id,
            block_height,
            amount,
            sender,
            receiver_address,
            nonce,
        })
    }

    fn read_u64(data: &[u8], offset: &mut usize) -> Option<u64> {
        if *offset + 8 > data.len() { return None; }
        let val = u64::from_le_bytes(data[*offset..*offset + 8].try_into().ok()?);
        *offset += 8;
        Some(val)
    }

    fn read_string(data: &[u8], offset: &mut usize) -> Option<String> {
        if *offset + 4 > data.len() { return None; }
        let len = u32::from_le_bytes(data[*offset..*offset + 4].try_into().ok()?) as usize;
        *offset += 4;
        if *offset + len > data.len() { return None; }
        let s = String::from_utf8(data[*offset..*offset + len].to_vec()).ok()?;
        *offset += len;
        Some(s)
    }
}

/// Save parsed event to file queue (same format as e2s-listener)
fn save_to_queue(event: &StakeEventData, queue_dir: &Path) -> Result<()> {
    let queue_file = queue_dir.join(format!("event_{}.json", event.nonce));
    let json = serde_json::to_string_pretty(event)?;
    std::fs::write(&queue_file, json)?;
    info!(nonce = event.nonce, path = %queue_file.display(), "Event saved to queue");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn build_test_config() -> ListenerConfig {
        shared::Config {
            target_chain: shared::config::ChainConfig {
                name: "1024chain-test".to_string(),
                chain_id: 91024,
                rpc_url: "http://localhost:8899".to_string(),
                contract_address: "7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E".to_string(),
                confirmation_blocks: None,
                commitment: Some("confirmed".to_string()),
                usdc_mint: None,
                ws_url: None,
            },
            ..shared::Config::default()
        }
    }

    /// Encode a mock StakeEvent as Anchor base64 data
    fn encode_stake_event(
        source_contract: &str,
        target_contract: &str,
        chain_id: u64,
        block_height: u64,
        amount: u64,
        sender: &str,
        receiver_address: &str,
        nonce: u64,
    ) -> String {
        let mut data = Vec::new();

        // Discriminator
        let mut hasher = Sha256::new();
        hasher.update(b"event:StakeEvent");
        let disc = &hasher.finalize()[..8];
        data.extend_from_slice(disc);

        // Fields
        fn write_string(buf: &mut Vec<u8>, s: &str) {
            buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
        }

        write_string(&mut data, source_contract);
        write_string(&mut data, target_contract);
        data.extend_from_slice(&chain_id.to_le_bytes());
        data.extend_from_slice(&block_height.to_le_bytes());
        data.extend_from_slice(&amount.to_le_bytes());
        write_string(&mut data, sender);
        write_string(&mut data, receiver_address);
        data.extend_from_slice(&nonce.to_le_bytes());

        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&data)
    }

    /// REL-T003: Solana sender address fills 32 bytes directly
    #[test]
    fn test_parse_solana_stake_event() {
        let config = build_test_config();
        let sender_pubkey = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
        let receiver = "CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC";

        let b64 = encode_stake_event(
            "CKRCgMnF7wgsrYFc4FT2WZYiWy3NQpCd3KGjvQWHruMS",
            "7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E",
            1,       // chain_id (Solana)
            12345,   // block_height
            5000000, // amount (5 USDC after fee deduction)
            sender_pubkey,
            receiver,
            1,
        );

        let event = anchor_event_parser::parse_anchor_event(&b64, &config);
        assert!(event.is_some(), "Should parse StakeEvent");
        let event = event.unwrap();

        assert_eq!(event.nonce, 1);
        assert_eq!(event.amount, 5_000_000);
        assert_eq!(event.sender, sender_pubkey);
        assert_eq!(event.receiver_address, receiver);
        assert_eq!(event.source_chain_id, 1);
        assert_eq!(event.target_chain_id, 91024);

        // Verify to_compact produces 32-byte sender
        let compact = event.to_compact().expect("to_compact should work for Solana address");
        let decoded_sender = bs58::decode(sender_pubkey).into_vec().unwrap();
        assert_eq!(compact.sender.len(), 32);
        assert_eq!(&compact.sender[..], &decoded_sender[..]);
    }

    #[test]
    fn test_parse_invalid_data() {
        let config = build_test_config();
        // Random base64 that doesn't match discriminator
        let result = anchor_event_parser::parse_anchor_event("AAAAAAAAAA==", &config);
        assert!(result.is_none());
    }

    #[test]
    fn test_save_and_read_queue() {
        let dir = std::env::temp_dir().join("sol2svm_test_queue");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let event = StakeEventData {
            source_contract: "CKRCgMnF7wgsrYFc4FT2WZYiWy3NQpCd3KGjvQWHruMS".to_string(),
            target_contract: "7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E".to_string(),
            source_chain_id: 1,
            target_chain_id: 91024,
            block_height: 100,
            amount: 5_000_000,
            sender: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            receiver_address: "CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC".to_string(),
            nonce: 42,
        };

        save_to_queue(&event, &dir).unwrap();
        let file = dir.join("event_42.json");
        assert!(file.exists());

        let content = std::fs::read_to_string(&file).unwrap();
        let parsed: StakeEventData = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.nonce, 42);
        assert_eq!(parsed.amount, 5_000_000);
        assert_eq!(parsed.sender, "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
