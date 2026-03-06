mod config;
mod listener;

use anyhow::Result;
use shared::logger;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::load_config()?;
    logger::init_logger(&config.logging.level, &config.logging.format)?;
    info!("Starting sol2svm-listener service");
    listener::start_listener(config).await?;
    Ok(())
}
