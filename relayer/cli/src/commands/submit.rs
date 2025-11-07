use anyhow::{Result, Context};
use ethers::prelude::*;
use tracing::{info, warn};

pub async fn submit_vaa(
    chain: &str,
    rpc_url: &str,
    vaa: &str,
    private_key: &str,
    contract_address: Option<&str>,
) -> Result<()> {
    match chain.to_lowercase().as_str() {
        "evm" | "ethereum" => {
            submit_vaa_evm(rpc_url, vaa, private_key, contract_address).await
        }
        "solana" => {
            submit_vaa_solana(rpc_url, vaa, private_key).await
        }
        _ => {
            anyhow::bail!("Unsupported chain: {}. Use 'evm' or 'solana'", chain);
        }
    }
}

async fn submit_vaa_evm(
    rpc_url: &str,
    vaa: &str,
    private_key: &str,
    contract_address: Option<&str>,
) -> Result<()> {
    info!("📡 Submitting VAA to EVM chain...");
    
    let contract_addr = contract_address
        .context("--contract address is required for EVM chains")?;
    
    info!("   RPC: {}", rpc_url);
    info!("   Contract: {}", contract_addr);
    
    // Parse VAA
    let vaa_bytes = if vaa.starts_with("0x") {
        hex::decode(&vaa[2..]).context("Failed to decode hex")?
    } else if std::path::Path::new(vaa).exists() {
        std::fs::read(vaa).context("Failed to read file")?
    } else {
        hex::decode(vaa).context("Failed to decode hex")?
    };
    
    info!("   VAA length: {} bytes", vaa_bytes.len());
    
    // Connect to provider
    let provider = Provider::<Http>::try_from(rpc_url)
        .context("Invalid RPC URL")?;
    
    // Create wallet
    let wallet: LocalWallet = private_key.parse()
        .context("Invalid private key")?;
    
    let client = SignerMiddleware::new(provider, wallet);
    
    // Parse contract address
    let contract: Address = contract_addr.parse()
        .context("Invalid contract address")?;
    
    info!("🔐 Wallet: {:?}", client.address());
    
    // Build transaction to call parseAndVerifyVAA
    let tx = TransactionRequest::new()
        .to(contract)
        .data(encode_parse_and_verify_vaa(&vaa_bytes));
    
    info!("📤 Sending transaction...");
    
    let pending_tx = client
        .send_transaction(tx, None)
        .await
        .context("Failed to send transaction")?;
    
    info!("   TX hash: {:?}", pending_tx.tx_hash());
    
    let receipt = pending_tx
        .await?
        .context("Transaction failed")?;
    
    if receipt.status == Some(1.into()) {
        info!("✅ VAA verification successful!");
        info!("   Block: {}", receipt.block_number.unwrap());
        info!("   Gas used: {}", receipt.gas_used.unwrap());
        Ok(())
    } else {
        anyhow::bail!("Transaction reverted");
    }
}

async fn submit_vaa_solana(
    rpc_url: &str,
    vaa: &str,
    keypair_path: &str,
) -> Result<()> {
    info!("📡 Submitting VAA to Solana...");
    info!("   RPC: {}", rpc_url);
    info!("   Keypair: {}", keypair_path);
    
    // Parse VAA
    let vaa_bytes = if vaa.starts_with("0x") {
        hex::decode(&vaa[2..]).context("Failed to decode hex")?
    } else if std::path::Path::new(vaa).exists() {
        std::fs::read(vaa).context("Failed to read file")?
    } else {
        hex::decode(vaa).context("Failed to decode hex")?
    };
    
    info!("   VAA length: {} bytes", vaa_bytes.len());
    
    // Parse VAA fields
    let vaa_version = vaa_bytes[0];
    let guardian_set_index = u32::from_be_bytes([
        vaa_bytes[1], vaa_bytes[2], vaa_bytes[3], vaa_bytes[4]
    ]);
    let signatures_len = vaa_bytes[5];
    
    // Body offset = 6 + (66 * signatures_len)
    let body_offset = 6 + (66 * signatures_len as usize);
    
    // Parse body
    let timestamp = u32::from_be_bytes([
        vaa_bytes[body_offset],
        vaa_bytes[body_offset + 1],
        vaa_bytes[body_offset + 2],
        vaa_bytes[body_offset + 3],
    ]);
    
    let nonce = u32::from_be_bytes([
        vaa_bytes[body_offset + 4],
        vaa_bytes[body_offset + 5],
        vaa_bytes[body_offset + 6],
        vaa_bytes[body_offset + 7],
    ]);
    
    let emitter_chain = u16::from_be_bytes([
        vaa_bytes[body_offset + 8],
        vaa_bytes[body_offset + 9],
    ]);
    
    let mut emitter_address = [0u8; 32];
    emitter_address.copy_from_slice(&vaa_bytes[body_offset + 10..body_offset + 42]);
    
    let sequence = u64::from_be_bytes([
        vaa_bytes[body_offset + 42],
        vaa_bytes[body_offset + 43],
        vaa_bytes[body_offset + 44],
        vaa_bytes[body_offset + 45],
        vaa_bytes[body_offset + 46],
        vaa_bytes[body_offset + 47],
        vaa_bytes[body_offset + 48],
        vaa_bytes[body_offset + 49],
    ]);
    
    let consistency_level = vaa_bytes[body_offset + 50];
    let payload = vaa_bytes[body_offset + 51..].to_vec();
    
    info!("📋 VAA Details:");
    info!("   Version: {}", vaa_version);
    info!("   Guardian Set: {}", guardian_set_index);
    info!("   Signatures: {}", signatures_len);
    info!("   Chain: {}", emitter_chain);
    info!("   Sequence: {}", sequence);
    info!("   Payload: {} bytes", payload.len());
    
    info!("✅ VAA parsed successfully");
    info!("📝 To submit to Solana, use:");
    info!("   anchor run post-vaa --provider.cluster localnet");
    info!("   (Full implementation requires Anchor client integration)");
    
    Ok(())
}

fn encode_parse_and_verify_vaa(vaa_bytes: &[u8]) -> Vec<u8> {
    use ethers::abi::{encode, Token};
    
    // Function selector for parseAndVerifyVAA(bytes)
    let selector = &ethers::utils::keccak256(b"parseAndVerifyVAA(bytes)")[0..4];
    
    // Encode parameters
    let encoded_params = encode(&[Token::Bytes(vaa_bytes.to_vec())]);
    
    [selector, &encoded_params].concat()
}


        vaa_bytes[1], vaa_bytes[2], vaa_bytes[3], vaa_bytes[4]
    ]);
    let signatures_len = vaa_bytes[5];
    
    // Body offset = 6 + (66 * signatures_len)
    let body_offset = 6 + (66 * signatures_len as usize);
    
    // Parse body
    let timestamp = u32::from_be_bytes([
        vaa_bytes[body_offset],
        vaa_bytes[body_offset + 1],
        vaa_bytes[body_offset + 2],
        vaa_bytes[body_offset + 3],
    ]);
    
    let nonce = u32::from_be_bytes([
        vaa_bytes[body_offset + 4],
        vaa_bytes[body_offset + 5],
        vaa_bytes[body_offset + 6],
        vaa_bytes[body_offset + 7],
    ]);
    
    let emitter_chain = u16::from_be_bytes([
        vaa_bytes[body_offset + 8],
        vaa_bytes[body_offset + 9],
    ]);
    
    let mut emitter_address = [0u8; 32];
    emitter_address.copy_from_slice(&vaa_bytes[body_offset + 10..body_offset + 42]);
    
    let sequence = u64::from_be_bytes([
        vaa_bytes[body_offset + 42],
        vaa_bytes[body_offset + 43],
        vaa_bytes[body_offset + 44],
        vaa_bytes[body_offset + 45],
        vaa_bytes[body_offset + 46],
        vaa_bytes[body_offset + 47],
        vaa_bytes[body_offset + 48],
        vaa_bytes[body_offset + 49],
    ]);
    
    let consistency_level = vaa_bytes[body_offset + 50];
    let payload = vaa_bytes[body_offset + 51..].to_vec();
    
    info!("📋 VAA Details:");
    info!("   Version: {}", vaa_version);
    info!("   Guardian Set: {}", guardian_set_index);
    info!("   Signatures: {}", signatures_len);
    info!("   Chain: {}", emitter_chain);
    info!("   Sequence: {}", sequence);
    info!("   Payload: {} bytes", payload.len());
    
    info!("✅ VAA parsed successfully");
    info!("📝 To submit to Solana, use:");
    info!("   anchor run post-vaa --provider.cluster localnet");
    info!("   (Full implementation requires Anchor client integration)");
    
    Ok(())
}

fn encode_parse_and_verify_vaa(vaa_bytes: &[u8]) -> Vec<u8> {
    use ethers::abi::{encode, Token};
    
    // Function selector for parseAndVerifyVAA(bytes)
    let selector = &ethers::utils::keccak256(b"parseAndVerifyVAA(bytes)")[0..4];
    
    // Encode parameters
    let encoded_params = encode(&[Token::Bytes(vaa_bytes.to_vec())]);
    
    [selector, &encoded_params].concat()
}


        vaa_bytes[1], vaa_bytes[2], vaa_bytes[3], vaa_bytes[4]
    ]);
    let signatures_len = vaa_bytes[5];
    
    // Body offset = 6 + (66 * signatures_len)
    let body_offset = 6 + (66 * signatures_len as usize);
    
    // Parse body
    let timestamp = u32::from_be_bytes([
        vaa_bytes[body_offset],
        vaa_bytes[body_offset + 1],
        vaa_bytes[body_offset + 2],
        vaa_bytes[body_offset + 3],
    ]);
    
    let nonce = u32::from_be_bytes([
        vaa_bytes[body_offset + 4],
        vaa_bytes[body_offset + 5],
        vaa_bytes[body_offset + 6],
        vaa_bytes[body_offset + 7],
    ]);
    
    let emitter_chain = u16::from_be_bytes([
        vaa_bytes[body_offset + 8],
        vaa_bytes[body_offset + 9],
    ]);
    
    let mut emitter_address = [0u8; 32];
    emitter_address.copy_from_slice(&vaa_bytes[body_offset + 10..body_offset + 42]);
    
    let sequence = u64::from_be_bytes([
        vaa_bytes[body_offset + 42],
        vaa_bytes[body_offset + 43],
        vaa_bytes[body_offset + 44],
        vaa_bytes[body_offset + 45],
        vaa_bytes[body_offset + 46],
        vaa_bytes[body_offset + 47],
        vaa_bytes[body_offset + 48],
        vaa_bytes[body_offset + 49],
    ]);
    
    let consistency_level = vaa_bytes[body_offset + 50];
    let payload = vaa_bytes[body_offset + 51..].to_vec();
    
    info!("📋 VAA Details:");
    info!("   Version: {}", vaa_version);
    info!("   Guardian Set: {}", guardian_set_index);
    info!("   Signatures: {}", signatures_len);
    info!("   Chain: {}", emitter_chain);
    info!("   Sequence: {}", sequence);
    info!("   Payload: {} bytes", payload.len());
    
    info!("✅ VAA parsed successfully");
    info!("📝 To submit to Solana, use:");
    info!("   anchor run post-vaa --provider.cluster localnet");
    info!("   (Full implementation requires Anchor client integration)");
    
    Ok(())
}

fn encode_parse_and_verify_vaa(vaa_bytes: &[u8]) -> Vec<u8> {
    use ethers::abi::{encode, Token};
    
    // Function selector for parseAndVerifyVAA(bytes)
    let selector = &ethers::utils::keccak256(b"parseAndVerifyVAA(bytes)")[0..4];
    
    // Encode parameters
    let encoded_params = encode(&[Token::Bytes(vaa_bytes.to_vec())]);
    
    [selector, &encoded_params].concat()
}


        vaa_bytes[1], vaa_bytes[2], vaa_bytes[3], vaa_bytes[4]
    ]);
    let signatures_len = vaa_bytes[5];
    
    // Body offset = 6 + (66 * signatures_len)
    let body_offset = 6 + (66 * signatures_len as usize);
    
    // Parse body
    let timestamp = u32::from_be_bytes([
        vaa_bytes[body_offset],
        vaa_bytes[body_offset + 1],
        vaa_bytes[body_offset + 2],
        vaa_bytes[body_offset + 3],
    ]);
    
    let nonce = u32::from_be_bytes([
        vaa_bytes[body_offset + 4],
        vaa_bytes[body_offset + 5],
        vaa_bytes[body_offset + 6],
        vaa_bytes[body_offset + 7],
    ]);
    
    let emitter_chain = u16::from_be_bytes([
        vaa_bytes[body_offset + 8],
        vaa_bytes[body_offset + 9],
    ]);
    
    let mut emitter_address = [0u8; 32];
    emitter_address.copy_from_slice(&vaa_bytes[body_offset + 10..body_offset + 42]);
    
    let sequence = u64::from_be_bytes([
        vaa_bytes[body_offset + 42],
        vaa_bytes[body_offset + 43],
        vaa_bytes[body_offset + 44],
        vaa_bytes[body_offset + 45],
        vaa_bytes[body_offset + 46],
        vaa_bytes[body_offset + 47],
        vaa_bytes[body_offset + 48],
        vaa_bytes[body_offset + 49],
    ]);
    
    let consistency_level = vaa_bytes[body_offset + 50];
    let payload = vaa_bytes[body_offset + 51..].to_vec();
    
    info!("📋 VAA Details:");
    info!("   Version: {}", vaa_version);
    info!("   Guardian Set: {}", guardian_set_index);
    info!("   Signatures: {}", signatures_len);
    info!("   Chain: {}", emitter_chain);
    info!("   Sequence: {}", sequence);
    info!("   Payload: {} bytes", payload.len());
    
    info!("✅ VAA parsed successfully");
    info!("📝 To submit to Solana, use:");
    info!("   anchor run post-vaa --provider.cluster localnet");
    info!("   (Full implementation requires Anchor client integration)");
    
    Ok(())
}

fn encode_parse_and_verify_vaa(vaa_bytes: &[u8]) -> Vec<u8> {
    use ethers::abi::{encode, Token};
    
    // Function selector for parseAndVerifyVAA(bytes)
    let selector = &ethers::utils::keccak256(b"parseAndVerifyVAA(bytes)")[0..4];
    
    // Encode parameters
    let encoded_params = encode(&[Token::Bytes(vaa_bytes.to_vec())]);
    
    [selector, &encoded_params].concat()
}


        vaa_bytes[1], vaa_bytes[2], vaa_bytes[3], vaa_bytes[4]
    ]);
    let signatures_len = vaa_bytes[5];
    
    // Body offset = 6 + (66 * signatures_len)
    let body_offset = 6 + (66 * signatures_len as usize);
    
    // Parse body
    let timestamp = u32::from_be_bytes([
        vaa_bytes[body_offset],
        vaa_bytes[body_offset + 1],
        vaa_bytes[body_offset + 2],
        vaa_bytes[body_offset + 3],
    ]);
    
    let nonce = u32::from_be_bytes([
        vaa_bytes[body_offset + 4],
        vaa_bytes[body_offset + 5],
        vaa_bytes[body_offset + 6],
        vaa_bytes[body_offset + 7],
    ]);
    
    let emitter_chain = u16::from_be_bytes([
        vaa_bytes[body_offset + 8],
        vaa_bytes[body_offset + 9],
    ]);
    
    let mut emitter_address = [0u8; 32];
    emitter_address.copy_from_slice(&vaa_bytes[body_offset + 10..body_offset + 42]);
    
    let sequence = u64::from_be_bytes([
        vaa_bytes[body_offset + 42],
        vaa_bytes[body_offset + 43],
        vaa_bytes[body_offset + 44],
        vaa_bytes[body_offset + 45],
        vaa_bytes[body_offset + 46],
        vaa_bytes[body_offset + 47],
        vaa_bytes[body_offset + 48],
        vaa_bytes[body_offset + 49],
    ]);
    
    let consistency_level = vaa_bytes[body_offset + 50];
    let payload = vaa_bytes[body_offset + 51..].to_vec();
    
    info!("📋 VAA Details:");
    info!("   Version: {}", vaa_version);
    info!("   Guardian Set: {}", guardian_set_index);
    info!("   Signatures: {}", signatures_len);
    info!("   Chain: {}", emitter_chain);
    info!("   Sequence: {}", sequence);
    info!("   Payload: {} bytes", payload.len());
    
    info!("✅ VAA parsed successfully");
    info!("📝 To submit to Solana, use:");
    info!("   anchor run post-vaa --provider.cluster localnet");
    info!("   (Full implementation requires Anchor client integration)");
    
    Ok(())
}

fn encode_parse_and_verify_vaa(vaa_bytes: &[u8]) -> Vec<u8> {
    use ethers::abi::{encode, Token};
    
    // Function selector for parseAndVerifyVAA(bytes)
    let selector = &ethers::utils::keccak256(b"parseAndVerifyVAA(bytes)")[0..4];
    
    // Encode parameters
    let encoded_params = encode(&[Token::Bytes(vaa_bytes.to_vec())]);
    
    [selector, &encoded_params].concat()
}


        vaa_bytes[1], vaa_bytes[2], vaa_bytes[3], vaa_bytes[4]
    ]);
    let signatures_len = vaa_bytes[5];
    
    // Body offset = 6 + (66 * signatures_len)
    let body_offset = 6 + (66 * signatures_len as usize);
    
    // Parse body
    let timestamp = u32::from_be_bytes([
        vaa_bytes[body_offset],
        vaa_bytes[body_offset + 1],
        vaa_bytes[body_offset + 2],
        vaa_bytes[body_offset + 3],
    ]);
    
    let nonce = u32::from_be_bytes([
        vaa_bytes[body_offset + 4],
        vaa_bytes[body_offset + 5],
        vaa_bytes[body_offset + 6],
        vaa_bytes[body_offset + 7],
    ]);
    
    let emitter_chain = u16::from_be_bytes([
        vaa_bytes[body_offset + 8],
        vaa_bytes[body_offset + 9],
    ]);
    
    let mut emitter_address = [0u8; 32];
    emitter_address.copy_from_slice(&vaa_bytes[body_offset + 10..body_offset + 42]);
    
    let sequence = u64::from_be_bytes([
        vaa_bytes[body_offset + 42],
        vaa_bytes[body_offset + 43],
        vaa_bytes[body_offset + 44],
        vaa_bytes[body_offset + 45],
        vaa_bytes[body_offset + 46],
        vaa_bytes[body_offset + 47],
        vaa_bytes[body_offset + 48],
        vaa_bytes[body_offset + 49],
    ]);
    
    let consistency_level = vaa_bytes[body_offset + 50];
    let payload = vaa_bytes[body_offset + 51..].to_vec();
    
    info!("📋 VAA Details:");
    info!("   Version: {}", vaa_version);
    info!("   Guardian Set: {}", guardian_set_index);
    info!("   Signatures: {}", signatures_len);
    info!("   Chain: {}", emitter_chain);
    info!("   Sequence: {}", sequence);
    info!("   Payload: {} bytes", payload.len());
    
    info!("✅ VAA parsed successfully");
    info!("📝 To submit to Solana, use:");
    info!("   anchor run post-vaa --provider.cluster localnet");
    info!("   (Full implementation requires Anchor client integration)");
    
    Ok(())
}

fn encode_parse_and_verify_vaa(vaa_bytes: &[u8]) -> Vec<u8> {
    use ethers::abi::{encode, Token};
    
    // Function selector for parseAndVerifyVAA(bytes)
    let selector = &ethers::utils::keccak256(b"parseAndVerifyVAA(bytes)")[0..4];
    
    // Encode parameters
    let encoded_params = encode(&[Token::Bytes(vaa_bytes.to_vec())]);
    
    [selector, &encoded_params].concat()
}

