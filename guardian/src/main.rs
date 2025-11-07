use anyhow::Result;
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber;

use guardian::config::GuardianConfig;
use guardian::guardian::GuardianNode;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "configs/local.toml")]
    config: String,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(&cli.log_level)
        .init();

    info!("🚀 Starting Guardian Node...");

    // Load configuration
    info!("Loading configuration from: {}", cli.config);
    let config = GuardianConfig::load(&cli.config)?;
    
    info!("Guardian Index: {}", config.guardian.index);
    info!("EVM RPC: {}", config.chains.evm.rpc_url);
    info!("Solana RPC: {}", config.chains.solana.rpc_url);

    // Create and run guardian node
    let mut guardian = GuardianNode::new(config).await?;
    
    info!("✅ Guardian node initialized");
    info!("Starting main loop...");

    // Run the guardian node
    match guardian.run().await {
        Ok(_) => {
            info!("Guardian node stopped gracefully");
            Ok(())
        }
        Err(e) => {
            warn!("Guardian node stopped with error: {}", e);
            Err(e)
        }
    }
}

