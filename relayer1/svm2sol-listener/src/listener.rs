use crate::config::ListenerConfig;
use anyhow::{anyhow, Result};
use shared::types::StakeEventData;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, error, info, warn};
use serde::Deserialize;
use serde_json::json;
use futures::stream::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Debug, Deserialize)]
struct WsNotification {
    method: Option<String>,
    params: Option<WsNotificationParams>,
    result: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct WsNotificationParams {
    result: WsNotificationResult,
}

#[derive(Debug, Deserialize)]
struct WsNotificationResult {
    value: LogsValue,
}

#[derive(Debug, Deserialize)]
struct LogsValue {
    signature: String,
    err: Option<serde_json::Value>,
    logs: Vec<String>,
}

pub async fn start_listener(config: ListenerConfig) -> Result<()> {
    let ws_url = config.source_chain.ws_url();
    let program_id = &config.source_chain.contract_address;

    info!(
        ws = %ws_url,
        program = %program_id,
        "Starting SVM->Solana event listener (WebSocket)"
    );

    let processed_signatures = Arc::new(Mutex::new(HashSet::<String>::new()));

    let mut backoff = Duration::from_secs(2);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);

    loop {
        match run_ws_listener(&config, processed_signatures.clone()).await {
            Ok(_) => {
                info!("WebSocket listener ended normally, reconnecting...");
                backoff = Duration::from_secs(2);
            }
            Err(e) => {
                error!(error = %e, backoff_secs = backoff.as_secs(), "WebSocket listener error, reconnecting");
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn run_ws_listener(
    config: &ListenerConfig,
    processed_signatures: Arc<Mutex<HashSet<String>>>,
) -> Result<()> {
    let ws_url = config.source_chain.ws_url();
    let program_id = &config.source_chain.contract_address;
    let commitment = config.source_chain.commitment.as_deref().unwrap_or("confirmed");

    info!(ws = %ws_url, "Connecting WebSocket...");

    let url = url::Url::parse(&ws_url)
        .map_err(|e| anyhow!("Invalid WebSocket URL '{}': {}", ws_url, e))?;

    let (ws_stream, _) = connect_async(url).await
        .map_err(|e| anyhow!("WebSocket connection failed: {}", e))?;

    info!("WebSocket connected");

    let (mut write, mut read) = ws_stream.split();

    let subscribe_msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "logsSubscribe",
        "params": [
            { "mentions": [program_id] },
            { "commitment": commitment }
        ]
    });

    use futures::SinkExt;
    write.send(Message::Text(subscribe_msg.to_string())).await
        .map_err(|e| anyhow!("Failed to send logsSubscribe: {}", e))?;

    debug!("Sent logsSubscribe for program {}", program_id);

    let mut subscription_id: Option<u64> = None;
    if let Some(Ok(msg)) = read.next().await {
        if let Message::Text(text) = msg {
            if let Ok(resp) = serde_json::from_str::<WsNotification>(&text) {
                if let Some(result) = resp.result {
                    subscription_id = result.as_u64();
                    info!(subscription_id = ?subscription_id, "logsSubscribe confirmed");
                }
            }
        }
    }

    if subscription_id.is_none() {
        return Err(anyhow!("Failed to confirm logsSubscribe subscription"));
    }

    info!("Listening for StakeEvents from 1024chain via WebSocket...");

    while let Some(msg_result) = read.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "WebSocket read error");
                return Err(anyhow!("WebSocket read error: {}", e));
            }
        };

        let text = match msg {
            Message::Text(t) => t,
            Message::Ping(_) => continue,
            Message::Pong(_) => continue,
            Message::Close(_) => {
                info!("WebSocket closed by server");
                return Ok(());
            }
            _ => continue,
        };

        let notification: WsNotification = match serde_json::from_str(&text) {
            Ok(n) => n,
            Err(e) => {
                debug!(error = %e, "Ignoring non-notification message");
                continue;
            }
        };

        if notification.method.as_deref() != Some("logsNotification") {
            continue;
        }

        let params = match notification.params {
            Some(p) => p,
            None => continue,
        };

        let value = params.result.value;

        if value.err.is_some() {
            debug!(signature = %value.signature, "Skipping failed transaction");
            continue;
        }

        {
            let processed = processed_signatures.lock().unwrap();
            if processed.contains(&value.signature) {
                continue;
            }
        }

        for log in &value.logs {
            if !log.contains("Program data:") {
                continue;
            }

            if let Some(event) = parse_stake_event(log, config) {
                info!(
                    signature = %value.signature,
                    nonce = event.nonce,
                    amount = event.amount,
                    receiver = %event.receiver_address,
                    "Captured StakeEvent from 1024chain"
                );

                match save_to_queue(&event, &config.queue.path) {
                    Ok(_) => {
                        let mut processed = processed_signatures.lock().unwrap();
                        processed.insert(value.signature.clone());
                        if processed.len() > 1000 {
                            processed.clear();
                        }
                    }
                    Err(e) => {
                        error!(
                            signature = %value.signature,
                            nonce = event.nonce,
                            error = %e,
                            "Failed to save event to queue"
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

fn parse_stake_event(log: &str, config: &ListenerConfig) -> Option<StakeEventData> {
    if let Some(data_str) = log.strip_prefix("Program data: ") {
        if let Ok(data) = base64_decode(data_str.trim()) {
            if data.len() > 8 {
                let event_data = &data[8..];
                if let Ok(event) = deserialize_anchor_event(event_data, config) {
                    return Some(event);
                }
            }
        }
    }
    None
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s)
        .map_err(|e| anyhow!("base64 decode failed: {}", e))
}

fn deserialize_anchor_event(data: &[u8], config: &ListenerConfig) -> Result<StakeEventData> {
    let mut offset = 0;

    let source_contract = read_string(data, &mut offset)?;
    let target_contract = read_string(data, &mut offset)?;
    let chain_id = read_u64(data, &mut offset)?;
    let block_height = read_u64(data, &mut offset)?;
    let amount = read_u64(data, &mut offset)?;
    let receiver_address = read_string(data, &mut offset)?;
    let nonce = read_u64(data, &mut offset)?;

    Ok(StakeEventData {
        source_contract,
        target_contract,
        source_chain_id: chain_id,
        target_chain_id: config.target_chain.chain_id,
        block_height,
        amount,
        sender: receiver_address.clone(),
        receiver_address,
        nonce,
    })
}

fn read_u64(data: &[u8], offset: &mut usize) -> Result<u64> {
    if *offset + 8 > data.len() {
        return Err(anyhow!("Buffer underflow reading u64"));
    }
    let val = u64::from_le_bytes(data[*offset..*offset + 8].try_into().unwrap());
    *offset += 8;
    Ok(val)
}

fn read_string(data: &[u8], offset: &mut usize) -> Result<String> {
    if *offset + 4 > data.len() {
        return Err(anyhow!("Buffer underflow reading string length"));
    }
    let len = u32::from_le_bytes(data[*offset..*offset + 4].try_into().unwrap()) as usize;
    *offset += 4;
    if *offset + len > data.len() {
        return Err(anyhow!("Buffer underflow reading string data"));
    }
    let s = String::from_utf8(data[*offset..*offset + len].to_vec())
        .map_err(|e| anyhow!("Invalid UTF-8: {}", e))?;
    *offset += len;
    Ok(s)
}

fn save_to_queue(event: &StakeEventData, queue_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(queue_dir)?;
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
                name: "solana-devnet".to_string(),
                chain_id: 103,
                rpc_url: "https://api.devnet.solana.com".to_string(),
                contract_address: "DtB1mvEcpWQdDxcmQPXjoe5dsrugBfU7NZjsLQwQ3KH5".to_string(),
                confirmation_blocks: None,
                commitment: Some("confirmed".to_string()),
                usdc_mint: None,
                ws_url: None,
            },
            ..shared::Config::default()
        }
    }

    fn encode_stake_event(
        source_contract: &str,
        target_contract: &str,
        chain_id: u64,
        block_height: u64,
        amount: u64,
        receiver_address: &str,
        nonce: u64,
    ) -> String {
        let mut data = Vec::new();

        let mut hasher = Sha256::new();
        hasher.update(b"event:StakeEvent");
        let disc = &hasher.finalize()[..8];
        data.extend_from_slice(disc);

        fn write_string(buf: &mut Vec<u8>, s: &str) {
            buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
        }

        write_string(&mut data, source_contract);
        write_string(&mut data, target_contract);
        data.extend_from_slice(&chain_id.to_le_bytes());
        data.extend_from_slice(&block_height.to_le_bytes());
        data.extend_from_slice(&amount.to_le_bytes());
        write_string(&mut data, receiver_address);
        data.extend_from_slice(&nonce.to_le_bytes());

        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&data)
    }

    #[test]
    fn test_parse_1024chain_stake_event() {
        let config = build_test_config();
        let receiver = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";

        let b64 = encode_stake_event(
            "7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E",
            "DtB1mvEcpWQdDxcmQPXjoe5dsrugBfU7NZjsLQwQ3KH5",
            91024,
            12345,
            5000000,
            receiver,
            1,
        );

        let log = format!("Program data: {}", b64);
        let event = parse_stake_event(&log, &config);
        assert!(event.is_some(), "Should parse StakeEvent");
        let event = event.unwrap();

        assert_eq!(event.nonce, 1);
        assert_eq!(event.amount, 5_000_000);
        assert_eq!(event.receiver_address, receiver);
        assert_eq!(event.source_chain_id, 91024);
        assert_eq!(event.target_chain_id, 103);
    }

    #[test]
    fn test_parse_invalid_data() {
        let config = build_test_config();
        let result = parse_stake_event("Program data: AAAAAAAAAA==", &config);
        assert!(result.is_none());
    }

    #[test]
    fn test_save_and_read_queue() {
        let dir = std::env::temp_dir().join("svm2sol_test_queue");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let event = StakeEventData {
            source_contract: "7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E".to_string(),
            target_contract: "DtB1mvEcpWQdDxcmQPXjoe5dsrugBfU7NZjsLQwQ3KH5".to_string(),
            source_chain_id: 91024,
            target_chain_id: 103,
            block_height: 100,
            amount: 5_000_000,
            sender: "CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC".to_string(),
            receiver_address: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            nonce: 42,
        };

        save_to_queue(&event, &dir).unwrap();
        let file = dir.join("event_42.json");
        assert!(file.exists());

        let content = std::fs::read_to_string(&file).unwrap();
        let parsed: StakeEventData = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.nonce, 42);
        assert_eq!(parsed.amount, 5_000_000);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
