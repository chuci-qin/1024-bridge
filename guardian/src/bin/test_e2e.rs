/// 端到端测试：监听事件 -> 签名 -> 输出
use anyhow::Result;
use tracing_subscriber;
use std::sync::Arc;
use tokio::sync::Mutex;

use guardian::config::EvmChainConfig;
use guardian::watcher::evm::EvmWatcher;
use guardian::signer::Signer;
use guardian::types::Observation;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("\n🧪 Guardian E2E Test: Watch -> Sign\n");
    println!("==================================\n");

    // Create signer (random for testing)
    println!("🔑 Creating signer...");
    let signer = Signer::generate_random(0)?;
    let signer_addr = hex::encode(signer.ethereum_address());
    println!("✅ Guardian address: {}\n", signer_addr);

    // Create watcher
    let config = EvmChainConfig {
        rpc_url: "ws://localhost:8545".to_string(),
        core_contract: "0x5FbDB2315678afecb367f032d93F642f64180aa3".to_string(),
        confirmations: 1,
    };

    println!("📡 Creating EVM watcher...");
    let mut watcher = EvmWatcher::new(&config).await?;
    println!("✅ Watcher ready\n");

    println!("👁️  Listening for events...");
    println!("   (Send a message to trigger signing)\n");

    // Create a channel for observations
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Observation>(100);
    
    // Spawn watcher task
    tokio::spawn(async move {
        // Simplified watch loop for testing
        let _ = watcher.watch().await;
    });

    // Wait for observations and sign them
    let mut message_count = 0;
    while let Some(observation) = rx.recv().await {
        message_count += 1;
        
        println!("📨 Received observation #{}", message_count);
        println!("   Chain: {}", observation.emitter_chain);
        println!("   Sequence: {}", observation.sequence);
        println!("   Nonce: {}", observation.nonce);
        
        // Sign the observation
        let signature = signer.sign_observation(&observation)?;
        
        println!("✍️  Generated signature:");
        println!("   Guardian Index: {}", signature.guardian_index);
        println!("   v: {}", signature.v);
        println!("   r: {}", hex::encode(&signature.r[..8]));
        println!("   s: {}", hex::encode(&signature.s[..8]));
        println!();
        
        // In real implementation, this would be broadcast to P2P network
        println!("📡 (Would broadcast to P2P network)");
        println!();
        
        if message_count >= 3 {
            println!("✅ Test complete! Processed {} messages\n", message_count);
            break;
        }
    }

    Ok(())
}

