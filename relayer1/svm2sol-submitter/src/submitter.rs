use crate::config::SubmitterConfig;
use crate::signer::Ed25519Signer;
use anyhow::{anyhow, Result};
use shared::types::{StakeEventData, CompactStakeEventData};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Signer,
    system_program,
    sysvar,
    transaction::Transaction,
};
use std::{path::Path, str::FromStr};
use tracing::{error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq)]
enum ErrorCategory {
    Retryable,
    NonRetryable,
}

pub async fn start_processor(config: SubmitterConfig) -> Result<()> {
    info!("Starting svm2sol event processor");

    let private_key = config
        .relayer
        .ed25519_private_key
        .as_deref()
        .ok_or_else(|| anyhow!("Ed25519 private key not configured"))?;
    let signer = Ed25519Signer::new(private_key)?;
    let rpc_client = RpcClient::new_with_commitment(
        config.target_chain.rpc_url.clone(),
        CommitmentConfig::confirmed(),
    );
    let program_id = Pubkey::from_str(&config.target_chain.contract_address)?;

    let usdc_mint = config
        .target_chain
        .usdc_mint
        .as_ref()
        .and_then(|m| Pubkey::from_str(m).ok())
        .ok_or_else(|| anyhow!("USDC mint address not configured in TARGET_CHAIN__USDC_MINT"))?;

    let mint_account = rpc_client
        .get_account(&usdc_mint)
        .map_err(|e| anyhow!("Failed to fetch USDC mint account: {}", e))?;
    let token_program_id = mint_account.owner;

    info!(
        relayer_pubkey = %signer.keypair().pubkey(),
        program_id = %program_id,
        token_program = %token_program_id,
        "svm2sol submitter initialized"
    );

    let queue_dir = &config.queue.path;
    std::fs::create_dir_all(queue_dir)?;
    info!(queue_path = %queue_dir.display(), "Queue directory initialized");

    loop {
        match process_queue(&config.queue.path, &signer, &rpc_client, &program_id, &usdc_mint, &token_program_id).await {
            Ok(processed) => {
                if processed > 0 {
                    info!(count = processed, "Processed events");
                }
            }
            Err(e) => {
                error!("Error processing queue: {}", e);
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

async fn process_queue(
    queue_dir: &Path,
    signer: &Ed25519Signer,
    rpc_client: &RpcClient,
    program_id: &Pubkey,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
) -> Result<usize> {
    let mut processed = 0;
    let entries = std::fs::read_dir(queue_dir)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    match serde_json::from_str::<StakeEventData>(&content) {
                        Ok(event) => {
                            info!(
                                nonce = event.nonce,
                                amount = event.amount,
                                sender = %event.sender,
                                receiver = %event.receiver_address,
                                "Processing 1024chain->Solana event"
                            );

                            match submit_signature(signer, rpc_client, program_id, usdc_mint, token_program_id, &event).await {
                                Ok(tx_signature) => {
                                    info!(nonce = event.nonce, tx = tx_signature, "Event processed successfully");
                                    if let Err(e) = std::fs::remove_file(&path) {
                                        warn!("Failed to remove processed file: {}", e);
                                    }
                                    processed += 1;
                                }
                                Err(e) => {
                                    let error_str = format!("{}", e);
                                    let category = categorize_error(&error_str);

                                    match category {
                                        ErrorCategory::NonRetryable => {
                                            warn!(nonce = event.nonce, error = %e, "Non-retryable error, removing event file");
                                            if let Err(re) = std::fs::remove_file(&path) {
                                                warn!("Failed to remove non-retryable event file: {}", re);
                                            }
                                        }
                                        ErrorCategory::Retryable => {
                                            error!(nonce = event.nonce, error = %e, "Retryable error, keeping file for retry");
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse event file {:?}: {}", path, e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read event file {:?}: {}", path, e);
                }
            }
        }
    }
    Ok(processed)
}

async fn submit_signature(
    signer: &Ed25519Signer,
    rpc_client: &RpcClient,
    program_id: &Pubkey,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    event: &StakeEventData,
) -> Result<String> {
    let compact_event = event.to_compact()
        .map_err(|e| anyhow!("Failed to convert to compact format: {}", e))?;

    info!(
        nonce = compact_event.nonce,
        amount = compact_event.amount,
        sender_hex = %format!("0x{}", hex::encode(compact_event.sender)),
        receiver_pubkey = %bs58::encode(compact_event.receiver_pubkey).into_string(),
        "Converted to compact format (88 bytes)"
    );

    let signature = signer.sign_compact_event(&compact_event)?;

    let (receiver_state, _) = Pubkey::find_program_address(&[b"receiver_state"], program_id);
    let (cross_chain_request, _) = Pubkey::find_program_address(
        &[b"cross_chain_request", &event.nonce.to_le_bytes()],
        program_id,
    );
    let (vault, _) = Pubkey::find_program_address(&[b"vault"], program_id);
    let vault_token_account =
        spl_associated_token_account::get_associated_token_address_with_program_id(&vault, usdc_mint, token_program_id);

    let ed25519_ix = create_ed25519_instruction(signer, &compact_event, &signature)?;

    let receiver_pubkey = solana_sdk::pubkey::Pubkey::try_from(compact_event.receiver_pubkey)
        .map_err(|e| anyhow!("Invalid receiver pubkey: {}", e))?;
    let receiver_token_account =
        spl_associated_token_account::get_associated_token_address_with_program_id(&receiver_pubkey, usdc_mint, token_program_id);

    let create_ata_ix = if rpc_client.get_account(&receiver_token_account).is_err() {
        Some(
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &signer.keypair().pubkey(),
                &receiver_pubkey,
                usdc_mint,
                token_program_id,
            )
        )
    } else {
        None
    };

    let submit_sig_ix = create_submit_signature_instruction(
        signer.keypair().pubkey(),
        program_id,
        &compact_event,
        &signature,
        receiver_state,
        cross_chain_request,
        vault,
        *usdc_mint,
        vault_token_account,
        receiver_token_account,
        *token_program_id,
    )?;

    let recent_blockhash = rpc_client
        .get_latest_blockhash()
        .map_err(|e| anyhow!("Failed to get latest blockhash: {}", e))?;

    let mut instructions = Vec::new();
    if let Some(ix) = create_ata_ix {
        instructions.push(ix);
    }
    instructions.push(ed25519_ix);
    instructions.push(submit_sig_ix);

    let mut transaction = Transaction::new_with_payer(
        &instructions,
        Some(&signer.keypair().pubkey()),
    );
    transaction.sign(&[signer.keypair()], recent_blockhash);

    info!(nonce = event.nonce, "Submitting transaction to Solana");

    match rpc_client.simulate_transaction(&transaction) {
        Ok(sim_result) => {
            if let Some(err) = sim_result.value.err {
                let msg = format!("Simulation failed: {:?}\nLogs: {:?}", err, sim_result.value.logs);
                return Err(anyhow!("{}", msg));
            }
            info!(nonce = event.nonce, "Simulation succeeded");
        }
        Err(e) => {
            warn!(nonce = event.nonce, error = %e, "Failed to simulate, proceeding");
        }
    }

    match rpc_client.send_and_confirm_transaction(&transaction) {
        Ok(sig) => {
            info!(nonce = event.nonce, tx = %sig, "Transaction confirmed on Solana");
            Ok(sig.to_string())
        }
        Err(e) => {
            Err(anyhow!("Failed to send transaction: {}", e))
        }
    }
}

fn create_ed25519_instruction(
    signer: &Ed25519Signer,
    event: &CompactStakeEventData,
    signature: &[u8],
) -> Result<Instruction> {
    use borsh::BorshSerialize;

    let mut message = Vec::new();
    event.nonce.serialize(&mut message)?;
    event.amount.serialize(&mut message)?;
    event.block_height.serialize(&mut message)?;
    event.sender.serialize(&mut message)?;
    event.receiver_pubkey.serialize(&mut message)?;

    let pubkey_bytes = signer.keypair().pubkey().to_bytes();

    const DATA_START: usize = 16;
    const PUBKEY_SIZE: usize = 32;
    const SIGNATURE_SIZE: usize = 64;

    let public_key_offset = DATA_START;
    let signature_offset = public_key_offset + PUBKEY_SIZE;
    let message_data_offset = signature_offset + SIGNATURE_SIZE;

    let mut instruction_data = Vec::with_capacity(
        DATA_START + PUBKEY_SIZE + SIGNATURE_SIZE + message.len()
    );

    instruction_data.push(1u8);
    instruction_data.push(0u8);
    instruction_data.extend_from_slice(&(signature_offset as u16).to_le_bytes());
    instruction_data.extend_from_slice(&u16::MAX.to_le_bytes());
    instruction_data.extend_from_slice(&(public_key_offset as u16).to_le_bytes());
    instruction_data.extend_from_slice(&u16::MAX.to_le_bytes());
    instruction_data.extend_from_slice(&(message_data_offset as u16).to_le_bytes());
    instruction_data.extend_from_slice(&(message.len() as u16).to_le_bytes());
    instruction_data.extend_from_slice(&u16::MAX.to_le_bytes());

    instruction_data.extend_from_slice(&pubkey_bytes);
    instruction_data.extend_from_slice(signature);
    instruction_data.extend_from_slice(&message);

    let ed25519_program_id = Pubkey::new_from_array([
        3, 125, 70, 214, 124, 147, 251, 190,
        18, 249, 66, 143, 131, 141, 64, 255,
        5, 112, 116, 73, 39, 244, 138, 100,
        252, 202, 112, 68, 128, 0, 0, 0,
    ]);

    Ok(Instruction {
        program_id: ed25519_program_id,
        accounts: vec![],
        data: instruction_data,
    })
}

#[allow(clippy::too_many_arguments)]
fn create_submit_signature_instruction(
    relayer_pubkey: Pubkey,
    program_id: &Pubkey,
    event: &CompactStakeEventData,
    signature: &[u8],
    receiver_state: Pubkey,
    cross_chain_request: Pubkey,
    vault: Pubkey,
    usdc_mint: Pubkey,
    vault_token_account: Pubkey,
    receiver_token_account: Pubkey,
    token_program_id: Pubkey,
) -> Result<Instruction> {
    use borsh::BorshSerialize;

    // Anchor discriminator for submit_signature
    // This must match the Solana bridge program's discriminator
    let discriminator: [u8; 8] = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"global:submit_signature");
        let hash = hasher.finalize();
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&hash[..8]);
        disc
    };

    let mut data = Vec::new();
    data.extend_from_slice(&discriminator);
    event.nonce.serialize(&mut data)?;

    event.nonce.serialize(&mut data)?;
    event.amount.serialize(&mut data)?;
    event.block_height.serialize(&mut data)?;
    event.sender.serialize(&mut data)?;
    event.receiver_pubkey.serialize(&mut data)?;

    signature.to_vec().serialize(&mut data)?;

    let accounts = vec![
        AccountMeta::new(receiver_state, false),
        AccountMeta::new(cross_chain_request, false),
        AccountMeta::new(relayer_pubkey, true),
        AccountMeta::new(vault, false),
        AccountMeta::new_readonly(usdc_mint, false),
        AccountMeta::new(vault_token_account, false),
        AccountMeta::new(receiver_token_account, false),
        AccountMeta::new_readonly(sysvar::instructions::ID, false),
        AccountMeta::new_readonly(token_program_id, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ];

    Ok(Instruction {
        program_id: *program_id,
        accounts,
        data,
    })
}

fn categorize_error(error_str: &str) -> ErrorCategory {
    if let Some(start) = error_str.find("Custom(") {
        if let Some(end) = error_str[start..].find(')') {
            if let Ok(code) = error_str[start + 7..start + end].parse::<u32>() {
                if (6000..7000).contains(&code) {
                    return ErrorCategory::NonRetryable;
                }
            }
        }
    }

    let error_lower = error_str.to_lowercase();
    if error_lower.contains("custom program error") {
        return ErrorCategory::NonRetryable;
    }

    ErrorCategory::Retryable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_categorize_error_retryable() {
        assert_eq!(categorize_error("connection timeout"), ErrorCategory::Retryable);
        assert_eq!(categorize_error("RPC error"), ErrorCategory::Retryable);
    }

    #[test]
    fn test_categorize_error_non_retryable() {
        assert_eq!(categorize_error("Custom(6005)"), ErrorCategory::NonRetryable);
        assert_eq!(categorize_error("custom program error: 0x1775"), ErrorCategory::NonRetryable);
    }

    #[test]
    fn test_to_compact_1024chain_sender() {
        let event = StakeEventData {
            source_contract: "7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E".to_string(),
            target_contract: "DtB1mvEcpWQdDxcmQPXjoe5dsrugBfU7NZjsLQwQ3KH5".to_string(),
            source_chain_id: 91024,
            target_chain_id: 103,
            block_height: 100,
            amount: 5_000_000,
            sender: "CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC".to_string(),
            receiver_address: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            nonce: 1,
        };

        let compact = event.to_compact().expect("Should convert 1024chain sender");
        assert_eq!(compact.sender.len(), 32);
        assert_eq!(compact.nonce, 1);
        assert_eq!(compact.amount, 5_000_000);
    }
}
