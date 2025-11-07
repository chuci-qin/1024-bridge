/// Solana Watcher 测试程序
use anyhow::Result;
use tracing_subscriber;

use guardian::config::SolanaChainConfig;
use guardian::watcher::solana::SolanaWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("\n🧪 Testing Solana Watcher...\n");

    // Create config
    let config = SolanaChainConfig {
        rpc_url: "http://localhost:8899".to_string(),
        ws_url: "ws://localhost:8900".to_string(),
        core_program: "9xd2UxwSv9qSMw3NXiFiXg4Th3oLmcYmMWe9KtRN9DHR".to_string(),
        commitment: "confirmed".to_string(),
    };

    // Create watcher
    println!("📡 Creating Solana watcher...");
    let mut watcher = SolanaWatcher::new(&config).await?;

    println!("✅ Watcher created successfully!\n");
    println!("📊 Monitoring Solana program: {}", config.core_program);
    println!("   RPC: {}", config.rpc_url);
    println!("   Mode: HTTP polling (every 2s)");
    println!("\nPress Ctrl+C to stop\n");

    // Create channel
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Start watching in background
    tokio::spawn(async move {
        let _ = watcher.watch_and_send(tx).await;
    });

    // Receive and print observations
    while let Some(obs) = rx.recv().await {
        println!("📨 Received Solana observation:");
        println!("   Chain: {}", obs.emitter_chain);
        println!("   Slot: {}", obs.block_number);
        println!("   Sequence: {}", obs.sequence);
        println!("   Signature: {}", obs.tx_hash);
        println!();
    }

    Ok(())
}

use anyhow::Result;
use tracing_subscriber;

use guardian::config::SolanaChainConfig;
use guardian::watcher::solana::SolanaWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("\n🧪 Testing Solana Watcher...\n");

    // Create config
    let config = SolanaChainConfig {
        rpc_url: "http://localhost:8899".to_string(),
        ws_url: "ws://localhost:8900".to_string(),
        core_program: "9xd2UxwSv9qSMw3NXiFiXg4Th3oLmcYmMWe9KtRN9DHR".to_string(),
        commitment: "confirmed".to_string(),
    };

    // Create watcher
    println!("📡 Creating Solana watcher...");
    let mut watcher = SolanaWatcher::new(&config).await?;

    println!("✅ Watcher created successfully!\n");
    println!("📊 Monitoring Solana program: {}", config.core_program);
    println!("   RPC: {}", config.rpc_url);
    println!("   Mode: HTTP polling (every 2s)");
    println!("\nPress Ctrl+C to stop\n");

    // Create channel
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Start watching in background
    tokio::spawn(async move {
        let _ = watcher.watch_and_send(tx).await;
    });

    // Receive and print observations
    while let Some(obs) = rx.recv().await {
        println!("📨 Received Solana observation:");
        println!("   Chain: {}", obs.emitter_chain);
        println!("   Slot: {}", obs.block_number);
        println!("   Sequence: {}", obs.sequence);
        println!("   Signature: {}", obs.tx_hash);
        println!();
    }

    Ok(())
}

use anyhow::Result;
use tracing_subscriber;

use guardian::config::SolanaChainConfig;
use guardian::watcher::solana::SolanaWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("\n🧪 Testing Solana Watcher...\n");

    // Create config
    let config = SolanaChainConfig {
        rpc_url: "http://localhost:8899".to_string(),
        ws_url: "ws://localhost:8900".to_string(),
        core_program: "9xd2UxwSv9qSMw3NXiFiXg4Th3oLmcYmMWe9KtRN9DHR".to_string(),
        commitment: "confirmed".to_string(),
    };

    // Create watcher
    println!("📡 Creating Solana watcher...");
    let mut watcher = SolanaWatcher::new(&config).await?;

    println!("✅ Watcher created successfully!\n");
    println!("📊 Monitoring Solana program: {}", config.core_program);
    println!("   RPC: {}", config.rpc_url);
    println!("   Mode: HTTP polling (every 2s)");
    println!("\nPress Ctrl+C to stop\n");

    // Create channel
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Start watching in background
    tokio::spawn(async move {
        let _ = watcher.watch_and_send(tx).await;
    });

    // Receive and print observations
    while let Some(obs) = rx.recv().await {
        println!("📨 Received Solana observation:");
        println!("   Chain: {}", obs.emitter_chain);
        println!("   Slot: {}", obs.block_number);
        println!("   Sequence: {}", obs.sequence);
        println!("   Signature: {}", obs.tx_hash);
        println!();
    }

    Ok(())
}

use anyhow::Result;
use tracing_subscriber;

use guardian::config::SolanaChainConfig;
use guardian::watcher::solana::SolanaWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("\n🧪 Testing Solana Watcher...\n");

    // Create config
    let config = SolanaChainConfig {
        rpc_url: "http://localhost:8899".to_string(),
        ws_url: "ws://localhost:8900".to_string(),
        core_program: "9xd2UxwSv9qSMw3NXiFiXg4Th3oLmcYmMWe9KtRN9DHR".to_string(),
        commitment: "confirmed".to_string(),
    };

    // Create watcher
    println!("📡 Creating Solana watcher...");
    let mut watcher = SolanaWatcher::new(&config).await?;

    println!("✅ Watcher created successfully!\n");
    println!("📊 Monitoring Solana program: {}", config.core_program);
    println!("   RPC: {}", config.rpc_url);
    println!("   Mode: HTTP polling (every 2s)");
    println!("\nPress Ctrl+C to stop\n");

    // Create channel
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Start watching in background
    tokio::spawn(async move {
        let _ = watcher.watch_and_send(tx).await;
    });

    // Receive and print observations
    while let Some(obs) = rx.recv().await {
        println!("📨 Received Solana observation:");
        println!("   Chain: {}", obs.emitter_chain);
        println!("   Slot: {}", obs.block_number);
        println!("   Sequence: {}", obs.sequence);
        println!("   Signature: {}", obs.tx_hash);
        println!();
    }

    Ok(())
}

use anyhow::Result;
use tracing_subscriber;

use guardian::config::SolanaChainConfig;
use guardian::watcher::solana::SolanaWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("\n🧪 Testing Solana Watcher...\n");

    // Create config
    let config = SolanaChainConfig {
        rpc_url: "http://localhost:8899".to_string(),
        ws_url: "ws://localhost:8900".to_string(),
        core_program: "9xd2UxwSv9qSMw3NXiFiXg4Th3oLmcYmMWe9KtRN9DHR".to_string(),
        commitment: "confirmed".to_string(),
    };

    // Create watcher
    println!("📡 Creating Solana watcher...");
    let mut watcher = SolanaWatcher::new(&config).await?;

    println!("✅ Watcher created successfully!\n");
    println!("📊 Monitoring Solana program: {}", config.core_program);
    println!("   RPC: {}", config.rpc_url);
    println!("   Mode: HTTP polling (every 2s)");
    println!("\nPress Ctrl+C to stop\n");

    // Create channel
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Start watching in background
    tokio::spawn(async move {
        let _ = watcher.watch_and_send(tx).await;
    });

    // Receive and print observations
    while let Some(obs) = rx.recv().await {
        println!("📨 Received Solana observation:");
        println!("   Chain: {}", obs.emitter_chain);
        println!("   Slot: {}", obs.block_number);
        println!("   Sequence: {}", obs.sequence);
        println!("   Signature: {}", obs.tx_hash);
        println!();
    }

    Ok(())
}

use anyhow::Result;
use tracing_subscriber;

use guardian::config::SolanaChainConfig;
use guardian::watcher::solana::SolanaWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("\n🧪 Testing Solana Watcher...\n");

    // Create config
    let config = SolanaChainConfig {
        rpc_url: "http://localhost:8899".to_string(),
        ws_url: "ws://localhost:8900".to_string(),
        core_program: "9xd2UxwSv9qSMw3NXiFiXg4Th3oLmcYmMWe9KtRN9DHR".to_string(),
        commitment: "confirmed".to_string(),
    };

    // Create watcher
    println!("📡 Creating Solana watcher...");
    let mut watcher = SolanaWatcher::new(&config).await?;

    println!("✅ Watcher created successfully!\n");
    println!("📊 Monitoring Solana program: {}", config.core_program);
    println!("   RPC: {}", config.rpc_url);
    println!("   Mode: HTTP polling (every 2s)");
    println!("\nPress Ctrl+C to stop\n");

    // Create channel
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Start watching in background
    tokio::spawn(async move {
        let _ = watcher.watch_and_send(tx).await;
    });

    // Receive and print observations
    while let Some(obs) = rx.recv().await {
        println!("📨 Received Solana observation:");
        println!("   Chain: {}", obs.emitter_chain);
        println!("   Slot: {}", obs.block_number);
        println!("   Sequence: {}", obs.sequence);
        println!("   Signature: {}", obs.tx_hash);
        println!();
    }

    Ok(())
}

