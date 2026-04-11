use crate::config::S2EListenerConfig;
use anyhow::{anyhow, Result};
use shared::types::StakeEventData;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use borsh::BorshDeserialize;
use serde::Deserialize;
use serde_json::json;
use base64::{Engine as _, engine::general_purpose};
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

pub async fn start_listener(config: S2EListenerConfig) -> Result<()> {
    let ws_url = config.source_chain.ws_url();
    let program_id = &config.source_chain.contract_address;

    info!(
        ws = %ws_url,
        program = %program_id,
        "Starting SVM event listener (WebSocket)"
    );

    let queue_dir = &config.queue.path;
    std::fs::create_dir_all(queue_dir)?;
    info!(queue_path = %queue_dir.display(), "Queue directory initialized");

    let mut backoff = Duration::from_secs(2);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);

    loop {
        let session_start = std::time::Instant::now();

        match run_ws_listener(&config).await {
            Ok(_) => {
                info!("WebSocket listener ended normally, reconnecting...");
                backoff = Duration::from_secs(2);
            }
            Err(e) => {
                let session_duration = session_start.elapsed();
                if session_duration > Duration::from_secs(120) {
                    info!(
                        duration_secs = session_duration.as_secs(),
                        "Session was alive for a while, resetting backoff"
                    );
                    backoff = Duration::from_secs(2);
                }
                error!(
                    error = %e,
                    backoff_secs = backoff.as_secs(),
                    session_secs = session_duration.as_secs(),
                    "WebSocket listener error, reconnecting"
                );
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const PING_INTERVAL: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(90);

async fn run_ws_listener(config: &S2EListenerConfig) -> Result<()> {
    let ws_url = config.source_chain.ws_url();
    let program_id = &config.source_chain.contract_address;
    let commitment = config.source_chain.commitment.as_deref().unwrap_or("confirmed");

    info!(ws = %ws_url, "Connecting WebSocket...");

    let url = url::Url::parse(&ws_url)
        .map_err(|e| anyhow!("Invalid WebSocket URL '{}': {}", ws_url, e))?;

    let (ws_stream, _) = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(url))
        .await
        .map_err(|_| anyhow!("WebSocket connection timed out after {}s", CONNECT_TIMEOUT.as_secs()))?
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
    match tokio::time::timeout(Duration::from_secs(10), read.next()).await {
        Ok(Some(Ok(msg))) => {
            if let Message::Text(text) = msg {
                if let Ok(resp) = serde_json::from_str::<WsNotification>(&text) {
                    if let Some(result) = resp.result {
                        subscription_id = result.as_u64();
                        info!(subscription_id = ?subscription_id, "logsSubscribe confirmed");
                    }
                }
            }
        }
        Ok(Some(Err(e))) => return Err(anyhow!("WebSocket error during subscription: {}", e)),
        Ok(None) => return Err(anyhow!("WebSocket closed before subscription confirmed")),
        Err(_) => return Err(anyhow!("Timed out waiting for logsSubscribe confirmation")),
    }

    if subscription_id.is_none() {
        return Err(anyhow!("Failed to confirm logsSubscribe subscription"));
    }

    info!("Listening for StakeEvents via WebSocket...");

    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    ping_interval.tick().await;
    let mut last_msg_time = std::time::Instant::now();

    loop {
        tokio::select! {
            msg_opt = read.next() => {
                let msg_result = match msg_opt {
                    Some(r) => r,
                    None => {
                        info!("WebSocket stream ended");
                        return Ok(());
                    }
                };

                last_msg_time = std::time::Instant::now();

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

                        if let Err(e) = save_to_queue(&event, &config.queue.path) {
                            error!(nonce = event.nonce, error = %e, "Failed to save event to queue");
                        }
                    }
                }
            }

            _ = ping_interval.tick() => {
                if last_msg_time.elapsed() > READ_TIMEOUT {
                    return Err(anyhow!(
                        "No message received for {}s, connection likely dead",
                        last_msg_time.elapsed().as_secs()
                    ));
                }

                if let Err(e) = write.send(Message::Ping(vec![].into())).await {
                    return Err(anyhow!("Failed to send ping: {}", e));
                }
                debug!("Sent keepalive ping");
            }
        }
    }
}

fn parse_stake_event(log: &str, config: &S2EListenerConfig) -> Option<StakeEventData> {
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

fn deserialize_anchor_event(data: &[u8], config: &S2EListenerConfig) -> Result<StakeEventData> {
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

fn save_to_queue(event: &StakeEventData, queue_dir: &Path) -> Result<()> {
    let queue_file = queue_dir.join(format!("event_{}.json", event.nonce));
    let json = serde_json::to_string_pretty(event)?;
    std::fs::write(&queue_file, json)?;
    info!(nonce = event.nonce, path = %queue_file.display(), "Event saved to queue");
    Ok(())
}
