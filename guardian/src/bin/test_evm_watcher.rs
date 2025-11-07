use anyhow::Result;
use tracing_subscriber;

use guardian::config::{GuardianConfig, EvmChainConfig};
use guardian::watcher::evm::EvmWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("🧪 Testing EVM Watcher...\n");

    // Create config
    let config = EvmChainConfig {
        rpc_url: "ws://localhost:8545".to_string(),
        core_contract: "0x5FbDB2315678afecb367f032d93F642f64180aa3".to_string(),
        confirmations: 1,
    };

    // Create watcher
    println!("📡 Creating EVM watcher...");
    let mut watcher = EvmWatcher::new(&config).await?;

    println!("✅ Watcher created successfully!\n");
    println!("📊 Listening for events on {}", config.core_contract);
    println!("Press Ctrl+C to stop\n");

    // Start watching
    watcher.watch().await?;

    Ok(())
}

