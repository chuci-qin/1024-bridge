use crate::config::S2EConfig;
use crate::signer::EcdsaSigner;
use crate::submitter::EvmSubmitter;
use anyhow::{anyhow, Result};
use shared::types::StakeEventData;
use std::time::Duration;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};
use borsh::BorshDeserialize;
use serde::Deserialize;
use serde_json::json;
use base64::{Engine as _, engine::general_purpose};
use futures::stream::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// WebSocket logsNotification 消息结构
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

/// 启动 SVM 事件监听器（WebSocket logsSubscribe）
pub async fn start_listener(config: S2EConfig) -> Result<()> {
    let ws_url = config.source_chain.ws_url();
    let program_id = &config.source_chain.contract_address;

    info!(
        ws = %ws_url,
        program = %program_id,
        "Starting SVM event listener (WebSocket)"
    );

    let private_key = config.relayer.ecdsa_private_key
        .as_ref()
        .ok_or_else(|| anyhow!(
            "ECDSA private key not configured. Please set RELAYER__ECDSA_PRIVATE_KEY environment variable.\n\
            Example: RELAYER__ECDSA_PRIVATE_KEY=0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef\n\
            You can generate one using: openssl rand -hex 32"
        ))?;

    let signer = EcdsaSigner::new(private_key)
        .map_err(|e| anyhow!("Failed to create ECDSA signer: {}\n\
            Hint: ECDSA private key must be a 64-character hex string (32 bytes).\n\
            Generate one with: openssl rand -hex 32", e))?;
    let submitter = EvmSubmitter::new(
        &config.target_chain.rpc_url,
        &config.target_chain.contract_address,
        private_key,
        config.target_chain.chain_id,
    )
    .map_err(|e| anyhow!("Failed to create EVM submitter: {}", e))?;

    let processed_signatures = Arc::new(Mutex::new(HashSet::<String>::new()));

    let mut backoff = Duration::from_secs(2);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);

    loop {
        match run_ws_listener(&config, &signer, &submitter, processed_signatures.clone()).await {
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

/// Single WebSocket session: connect, subscribe, and process notifications until disconnect.
async fn run_ws_listener(
    config: &S2EConfig,
    signer: &EcdsaSigner,
    submitter: &EvmSubmitter,
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

    // Send logsSubscribe
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

    // Read subscription confirmation
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

    info!("Listening for StakeEvents via WebSocket...");

    // Process incoming notifications
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

        // Scan logs for Anchor events
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
                    "Captured StakeEvent"
                );

                info!(
                    source_contract = %event.source_contract,
                    target_contract = %event.target_contract,
                    source_chain_id = event.source_chain_id,
                    target_chain_id = event.target_chain_id,
                    block_height = event.block_height,
                    "Event details"
                );

                match process_event(config, event.clone(), signer, submitter).await {
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
                            "Failed to process event"
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

/// 解析 Anchor 事件日志
fn parse_stake_event(log: &str, config: &S2EConfig) -> Option<StakeEventData> {
    if let Some(data_str) = log.strip_prefix("Program data: ") {
        if let Ok(data) = general_purpose::STANDARD.decode(data_str.trim()) {
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

/// 反序列化 Anchor StakeEvent
fn deserialize_anchor_event(data: &[u8], config: &S2EConfig) -> Result<StakeEventData> {
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

    let anchor_event = AnchorStakeEvent::try_from_slice(data)?;

    Ok(StakeEventData {
        source_contract: anchor_event.source_contract,
        target_contract: anchor_event.target_contract,
        source_chain_id: anchor_event.chain_id,
        target_chain_id: config.target_chain.chain_id,
        block_height: anchor_event.block_height,
        amount: anchor_event.amount,
        sender: anchor_event.receiver_address.clone(),
        receiver_address: anchor_event.receiver_address,
        nonce: anchor_event.nonce,
    })
}

/// 处理单个事件
async fn process_event(
    _config: &S2EConfig,
    event: StakeEventData,
    signer: &EcdsaSigner,
    submitter: &EvmSubmitter,
) -> Result<()> {
    info!(nonce = event.nonce, "Processing event");

    let signature = signer.sign_event(&event)?;
    info!(nonce = event.nonce, "Generated signature");

    let tx_hash = submitter.submit_signature(&event, &signature).await?;
    info!(
        nonce = event.nonce,
        tx = tx_hash,
        "Submitted signature to EVM"
    );

    Ok(())
}
