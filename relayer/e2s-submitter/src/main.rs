mod api;
mod config;
mod signer;
mod submitter;

use anyhow::Result;
use shared::logger;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::load_config()?;

    let log_file = config.logging.log_file.clone()
        .unwrap_or_else(|| "./logs/e2s-submitter.log".to_string());
    let _log_guard = logger::init_logger_with_file(
        &config.logging.level,
        &config.logging.format,
        Some(&log_file),
    )?;
    info!("Starting e2s-submitter service");

    let api_handle = tokio::spawn(api::start_server(config.clone()));
    info!(port = config.api.port, "HTTP API server started");

    let processor_handle = tokio::spawn(submitter::start_processor(config.clone()));
    info!("Event processor started");

    tokio::select! {
        _ = api_handle => {
            info!("API server stopped");
        }
        _ = processor_handle => {
            info!("Event processor stopped");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    info!("e2s-submitter service stopped");
    Ok(())
}

