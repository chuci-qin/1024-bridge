mod api;
mod config;
mod signer;
mod submitter;

use anyhow::Result;
use shared::{logger, metrics};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::load_config()?;

    let log_file = config.logging.log_file.clone()
        .unwrap_or_else(|| "./logs/s2e-submitter.log".to_string());
    let _log_guard = logger::init_logger_with_file(
        &config.logging.level,
        &config.logging.format,
        Some(&log_file),
    )?;
    info!("Starting s2e-submitter service");
    info!(
        source = config.source_chain.name,
        target = config.target_chain.name,
        "Service configuration loaded"
    );

    metrics::init_metrics();
    config.validate()?;
    info!("Configuration validated");

    let api_config = config.clone();
    tokio::spawn(async move {
        match api::start_server(api_config).await {
            Ok(_) => info!("API server stopped gracefully"),
            Err(e) => tracing::error!("API server error: {}", e),
        }
    });
    info!(port = config.api.port, "HTTP API server started");

    info!("Event processor started");

    tokio::select! {
        result = submitter::start_processor(config) => {
            if let Err(e) = result {
                tracing::error!("Event processor returned error: {}", e);
            }
            info!("Event processor stopped");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    info!("s2e-submitter service stopped");
    Ok(())
}
