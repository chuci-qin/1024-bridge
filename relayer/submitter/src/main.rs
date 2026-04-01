use anyhow::Result;
use bridge1024_core::types::BridgeDirection;
use std::env;
use tracing_subscriber::{fmt, EnvFilter};

mod evm_submitter;
mod svm_submitter;

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .json()
        .init();

    let bridge_id = env::var("BRIDGE_ID").expect("BRIDGE_ID required");
    let config_path =
        env::var("CONFIG_PATH").unwrap_or_else(|_| "deploy/config/bridges.json".to_string());
    let queue_dir =
        env::var("QUEUE_DIR").unwrap_or_else(|_| format!("/data/{}/queue", bridge_id));
    let dead_letter_dir =
        env::var("DEAD_LETTER_DIR").unwrap_or_else(|_| format!("/data/{}/dead_letter", bridge_id));

    tracing::info!(bridge_id = %bridge_id, "Starting submitter");

    let config = bridge1024_core::config::BridgesConfig::load(&config_path)?;
    let bridge = config
        .bridges
        .get(&bridge_id)
        .ok_or_else(|| anyhow::anyhow!("Bridge '{}' not found in config", bridge_id))?;

    let direction = bridge.direction();
    let target = bridge
        .target_config()
        .ok_or_else(|| anyhow::anyhow!("No target chain config for bridge '{}'", bridge_id))?;

    std::fs::create_dir_all(&queue_dir)?;
    std::fs::create_dir_all(&dead_letter_dir)?;

    match direction {
        BridgeDirection::EvmToSvm | BridgeDirection::SvmToSvm => {
            tracing::info!(direction = %direction, "Starting SVM submitter");
            svm_submitter::run(target, &queue_dir, &dead_letter_dir, &bridge_id).await?;
        }
        BridgeDirection::SvmToEvm => {
            tracing::info!(direction = %direction, "Starting EVM submitter");
            evm_submitter::run(target, &queue_dir, &dead_letter_dir, &bridge_id).await?;
        }
    }

    Ok(())
}
