mod config;
mod listener;

use anyhow::Result;
use shared::{logger, metrics};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::load_config()?;

    let log_file = config.logging.log_file.clone()
        .unwrap_or_else(|| "./logs/s2e-listener.log".to_string());
    let _log_guard = logger::init_logger_with_file(
        &config.logging.level,
        &config.logging.format,
        Some(&log_file),
    )?;
    info!("Starting s2e-listener service");
    info!(
        source = config.source_chain.name,
        target = config.target_chain.name,
        "Service configuration loaded"
    );

    metrics::init_metrics();
    config.validate()?;
    info!("Configuration validated");

    tokio::select! {
        result = listener::start_listener(config) => {
            if let Err(e) = result {
                tracing::error!("Event listener returned error: {}", e);
            }
            info!("Event listener stopped");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    info!("s2e-listener service stopped");
    Ok(())
}
