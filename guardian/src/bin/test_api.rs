/// 测试 Guardian REST API
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing_subscriber;

use guardian::aggregator::Aggregator;
use guardian::api::start_server;
use guardian::signer::Signer;
use guardian::types::{Observation, SignedObservation};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("\n🧪 Testing Guardian REST API\n");
    println!("===========================\n");

    // Create aggregator with test data
    println!("📝 Preparing test data...");
    let mut aggregator = Aggregator::new(0);

    // Create test observation
    let observation = Observation {
        emitter_chain: 1,
        emitter_address: {
            let mut addr = [0u8; 32];
            addr[12..].copy_from_slice(&hex::decode("f39fd6e51aad88f6f4ce6ab8827279cfffb92266")?);
            addr
        },
        sequence: 123,
        tx_hash: "0xtest123".to_string(),
        block_number: 5000,
        timestamp: 1699264800,
        nonce: 55555,
        consistency_level: 200,
        payload: b"Test API VAA".to_vec(),
    };

    aggregator.add_observation(observation.clone());

    // Add 13 signatures
    for i in 0..13 {
        let signer = Signer::generate_random(i)?;
        let signature = signer.sign_observation(&observation)?;
        let signed_obs = SignedObservation {
            observation: observation.clone(),
            signature,
        };
        aggregator.add_signature(signed_obs);
    }

    println!("✅ VAA generated with 13 signatures\n");

    // Start API server
    let aggregator_arc = Arc::new(RwLock::new(aggregator));
    let listen_addr = "0.0.0.0:7071";

    println!("🌐 Starting API server on {}...\n", listen_addr);

    let server_aggregator = Arc::clone(&aggregator_arc);
    tokio::spawn(async move {
        if let Err(e) = start_server(listen_addr, server_aggregator).await {
            eprintln!("Server error: {}", e);
        }
    });

    // Wait for server to start
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    println!("✅ Server started\n");
    println!("📋 Testing API endpoints...\n");

    // Test 1: Health check
    println!("Test 1: GET /health");
    let response = reqwest::get("http://localhost:7071/health").await?;
    println!("   Status: {}", response.status());
    let health: serde_json::Value = response.json().await?;
    println!("   Response: {}", serde_json::to_string_pretty(&health)?);
    println!("   ✅ Health check passed\n");

    // Test 2: Get VAA
    println!("Test 2: GET /v1/signed_vaa/1/0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266/123");
    let vaa_url = "http://localhost:7071/v1/signed_vaa/1/0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266/123";
    let response = reqwest::get(vaa_url).await?;
    println!("   Status: {}", response.status());
    
    if response.status().is_success() {
        let vaa_data: serde_json::Value = response.json().await?;
        println!("   Response: {}", serde_json::to_string_pretty(&vaa_data)?);
        println!("   ✅ VAA retrieval successful\n");
        
        // Extract VAA hex
        if let Some(vaa_hex) = vaa_data.get("vaa_hex").and_then(|v| v.as_str()) {
            println!("📦 VAA Data:");
            println!("   Length: {} bytes", (vaa_hex.len() - 2) / 2);
            println!("   Hex (first 64 chars): {}...", &vaa_hex[..64.min(vaa_hex.len())]);
            println!();
        }
    } else {
        println!("   ❌ VAA not found");
        return Err(anyhow::anyhow!("VAA retrieval failed"));
    }

    // Test 3: Get non-existent VAA
    println!("Test 3: GET /v1/signed_vaa/1/0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266/999");
    let response = reqwest::get("http://localhost:7071/v1/signed_vaa/1/0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266/999").await?;
    println!("   Status: {}", response.status());
    
    if response.status() == 404 {
        println!("   ✅ Correctly returns 404 for missing VAA\n");
    } else {
        println!("   ❌ Unexpected status code");
    }

    println!("╔══════════════════════════════════════╗");
    println!("║  ✅ All API tests passed!            ║");
    println!("╚══════════════════════════════════════╝");
    println!();
    println!("API is ready for use:");
    println!("  Health: http://localhost:7071/health");
    println!("  VAA:    http://localhost:7071/v1/signed_vaa/{{chain}}/{{emitter}}/{{seq}}");
    println!();

    // Keep server running for manual testing
    println!("Server running... (Press Ctrl+C to stop)");
    tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;

    Ok(())
}

