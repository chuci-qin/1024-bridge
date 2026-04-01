use anyhow::Result;
use std::env;
use tracing_subscriber::{fmt, EnvFilter};

mod evm_listener;
mod svm_listener;

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .json()
        .init();

    let bridge_id = env::var("BRIDGE_ID").expect("BRIDGE_ID environment variable required");
    let config_path = env::var("CONFIG_PATH")
        .unwrap_or_else(|_| "deploy/config/bridges.json".to_string());
    let queue_dir =
        env::var("QUEUE_DIR").unwrap_or_else(|_| format!("/data/{}/queue", bridge_id));
    let checkpoint_path = env::var("CHECKPOINT_PATH")
        .unwrap_or_else(|_| format!("/data/{}/checkpoint.json", bridge_id));

    tracing::info!(bridge_id = %bridge_id, config = %config_path, "Starting listener");

    let config = bridge1024_core::config::BridgesConfig::load(&config_path)?;
    let bridge = config
        .bridges
        .get(&bridge_id)
        .ok_or_else(|| anyhow::anyhow!("Bridge '{}' not found in config", bridge_id))?;

    let direction = bridge.direction();

    std::fs::create_dir_all(&queue_dir)?;

    match direction {
        bridge1024_core::types::BridgeDirection::EvmToSvm => {
            tracing::info!("Starting EVM listener for {}", bridge_id);
            evm_listener::run(
                bridge.source_config(),
                bridge.target.chain_id,
                &queue_dir,
                &checkpoint_path,
                &bridge_id,
            )
            .await?;
        }
        bridge1024_core::types::BridgeDirection::SvmToEvm
        | bridge1024_core::types::BridgeDirection::SvmToSvm => {
            tracing::info!("Starting SVM listener for {}", bridge_id);
            svm_listener::run(
                bridge.source_config(),
                bridge.target.chain_id,
                &queue_dir,
                &checkpoint_path,
                &bridge_id,
            )
            .await?;
        }
    }

    Ok(())
}
