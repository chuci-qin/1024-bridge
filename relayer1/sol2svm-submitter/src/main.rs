mod config;
mod signer;
mod submitter;

use anyhow::Result;
use shared::logger;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::load_config()?;
    logger::init_logger(&config.logging.level, &config.logging.format)?;
    info!("Starting sol2svm-submitter service");

    let processor_handle = tokio::spawn(submitter::start_processor(config.clone()));
    info!("Event processor started");

    tokio::select! {
        _ = processor_handle => {
            info!("Event processor stopped");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    info!("sol2svm-submitter service stopped");
    Ok(())
}
