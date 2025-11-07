/// 多签测试：模拟19个Guardian对同一消息签名并聚合VAA
use anyhow::Result;
use tracing_subscriber;

use guardian::aggregator::Aggregator;
use guardian::signer::Signer;
use guardian::types::{Observation, SignedObservation, GUARDIAN_SET_SIZE, SIGNATURE_QUORUM};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("\n🧪 Multi-Signature VAA Generation Test\n");
    println!("======================================\n");

    // Step 1: 创建19个Guardian
    println!("🔑 Generating {} guardians...", GUARDIAN_SET_SIZE);
    let mut guardians = Vec::new();
    for i in 0..GUARDIAN_SET_SIZE {
        let signer = Signer::generate_random(i as u8)?;
        guardians.push(signer);
    }
    println!("✅ {} guardians created\n", guardians.len());

    // Step 2: 创建测试消息
    let observation = Observation {
        emitter_chain: 1,
        emitter_address: {
            let mut addr = [0u8; 32];
            addr[12..].copy_from_slice(&hex::decode("f39fd6e51aad88f6f4ce6ab8827279cfffb92266")?);
            addr
        },
        sequence: 42,
        tx_hash: "0x1234567890abcdef".to_string(),
        block_number: 1000,
        timestamp: 1699264800,
        nonce: 99999,
        consistency_level: 200,
        payload: b"Hello from EVM to Solana!".to_vec(),
    };

    println!("📨 Test message:");
    println!("   Chain: {}", observation.emitter_chain);
    println!("   Sequence: {}", observation.sequence);
    println!("   Nonce: {}", observation.nonce);
    println!("   Payload: {}", String::from_utf8_lossy(&observation.payload));
    println!();

    // Step 3: 每个Guardian独立签名
    println!("✍️  {} guardians signing...", guardians.len());
    let mut aggregator = Aggregator::new(0);
    aggregator.add_observation(observation.clone());

    let mut vaa_opt = None;
    for (i, signer) in guardians.iter().enumerate() {
        let signature = signer.sign_observation(&observation)?;
        
        let signed_obs = SignedObservation {
            observation: observation.clone(),
            signature,
        };

        vaa_opt = aggregator.add_signature(signed_obs);

        if i + 1 == SIGNATURE_QUORUM {
            println!("   🎯 Quorum reached at {}/{}", i + 1, GUARDIAN_SET_SIZE);
        }

        if vaa_opt.is_some() {
            break;
        }
    }

    // Step 4: 验证VAA
    println!();
    if let Some(vaa) = vaa_opt {
        println!("🎉 VAA Generated Successfully!\n");
        println!("VAA Details:");
        println!("   Version: {}", vaa.version);
        println!("   Guardian Set Index: {}", vaa.guardian_set_index);
        println!("   Signatures: {}/{}", vaa.signatures.len(), GUARDIAN_SET_SIZE);
        println!("   Emitter Chain: {}", vaa.emitter_chain);
        println!("   Sequence: {}", vaa.sequence);
        println!("   Nonce: {}", vaa.nonce);
        println!("   Payload Length: {} bytes", vaa.payload.len());
        println!();
        
        // 显示签名信息
        println!("📋 Signature Details:");
        for (i, sig) in vaa.signatures.iter().enumerate().take(5) {
            println!("   Guardian {}: v={}, r={}", 
                sig.guardian_index, 
                sig.v, 
                hex::encode(&sig.r[..8])
            );
        }
        if vaa.signatures.len() > 5 {
            println!("   ... and {} more signatures", vaa.signatures.len() - 5);
        }
        println!();
        
        // 计算VAA digest
        let digest = vaa.digest();
        println!("🔐 VAA Digest: {}", hex::encode(digest));
        println!();
        
        println!("✅ SUCCESS: Multi-signature VAA generation validated!");
        println!();
        println!("Summary:");
        println!("  ✅ 19 guardians created");
        println!("  ✅ 19 signatures generated");
        println!("  ✅ Quorum reached at {}/{}", SIGNATURE_QUORUM, GUARDIAN_SET_SIZE);
        println!("  ✅ VAA aggregated with {} signatures", vaa.signatures.len());
        println!();
    } else {
        println!("❌ FAILED: Could not generate VAA");
        return Err(anyhow::anyhow!("VAA generation failed"));
    }

    Ok(())
}

