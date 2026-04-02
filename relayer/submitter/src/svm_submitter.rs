use anyhow::{anyhow, Result};
use borsh::BorshSerialize;
use bridge1024_core::config::ChainConfig;
use bridge1024_core::crypto;
use bridge1024_core::types::{CompactStakeEventData, QueuedEvent};
use sha2::{Digest, Sha256};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{error, info, warn};

/// 0.05 SOL in lamports — warn if relayer balance drops below this.
const MIN_SOL_BALANCE_LAMPORTS: u64 = 50_000_000;

/// Default compute-unit limit per submit_signature transaction.
const COMPUTE_UNIT_LIMIT: u32 = 400_000;

pub async fn run(
    config: &ChainConfig,
    queue_dir: &str,
    dead_letter_dir: &str,
    bridge_id: &str,
) -> Result<()> {
    let private_key_str =
        std::env::var("RELAYER_PRIVATE_KEY").expect("RELAYER_PRIVATE_KEY required");

    let commitment = match config.commitment.as_deref().unwrap_or("finalized") {
        "processed" => CommitmentConfig::processed(),
        "confirmed" => CommitmentConfig::confirmed(),
        _ => CommitmentConfig::finalized(),
    };
    let client = RpcClient::new_with_commitment(config.rpc_url.clone(), commitment);

    let keypair = parse_keypair(&private_key_str)?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(
        keypair.to_bytes()[..32]
            .try_into()
            .map_err(|_| anyhow!("Keypair secret extraction failed"))?,
    );

    let program_id: Pubkey = std::env::var("PROGRAM_ID")
        .expect("PROGRAM_ID required")
        .parse()
        .map_err(|_| anyhow!("Invalid PROGRAM_ID"))?;

    let token_mint: Pubkey = config
        .token_address
        .parse()
        .map_err(|_| anyhow!("Invalid token_address in config: {}", config.token_address))?;

    let poll_interval = Duration::from_secs(2);
    let max_retries: u32 = std::env::var("MAX_RETRIES")
        .unwrap_or_else(|_| "10".to_string())
        .parse()?;
    let compute_unit_price: u64 = std::env::var("COMPUTE_UNIT_PRICE")
        .unwrap_or_else(|_| "1000".to_string())
        .parse()?;

    // Auto-detect token program (SPL Token vs Token-2022) from mint account owner
    let mint_account = client
        .get_account(&token_mint)
        .await
        .map_err(|e| anyhow!("Failed to fetch token mint account: {}", e))?;
    let token_program_id = mint_account.owner;

    info!(
        relayer = %keypair.pubkey(),
        program_id = %program_id,
        token_mint = %token_mint,
        token_program = %token_program_id,
        bridge_id = bridge_id,
        max_retries = max_retries,
        compute_unit_price = compute_unit_price,
        "SVM submitter initialized"
    );

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutdown signal received");
                break;
            }
            _ = tokio::time::sleep(poll_interval) => {
                check_balance(&client, &keypair).await;
                process_queue(
                    &client, &keypair, &signing_key, &program_id,
                    &token_mint, &token_program_id,
                    queue_dir, dead_letter_dir,
                    max_retries, compute_unit_price,
                ).await;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Balance monitoring
// ---------------------------------------------------------------------------

async fn check_balance(client: &RpcClient, keypair: &Keypair) {
    match client.get_balance(&keypair.pubkey()).await {
        Ok(lamports) => {
            if lamports < MIN_SOL_BALANCE_LAMPORTS {
                warn!(
                    balance_sol = lamports as f64 / 1_000_000_000.0,
                    "Low relayer SOL balance"
                );
            }
        }
        Err(e) => warn!(error = %e, "Failed to check SOL balance"),
    }
}

// ---------------------------------------------------------------------------
// Queue processing
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn process_queue(
    client: &RpcClient,
    keypair: &Keypair,
    signing_key: &ed25519_dalek::SigningKey,
    program_id: &Pubkey,
    token_mint: &Pubkey,
    token_program_id: &Pubkey,
    queue_dir: &str,
    dead_letter_dir: &str,
    max_retries: u32,
    compute_unit_price: u64,
) {
    let entries = match std::fs::read_dir(queue_dir) {
        Ok(e) => e,
        Err(e) => {
            error!(error = %e, "Failed to read queue directory");
            return;
        }
    };

    let mut events: Vec<(PathBuf, QueuedEvent)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<QueuedEvent>(&content) {
                Ok(queued) => events.push((path, queued)),
                Err(e) => warn!(path = %path.display(), error = %e, "Failed to parse event file"),
            },
            Err(e) => warn!(path = %path.display(), error = %e, "Failed to read event file"),
        }
    }

    events.sort_by_key(|(_, q)| q.event.nonce);

    for (path, _) in &events {
        if let Err(e) = process_event(
            client,
            keypair,
            signing_key,
            program_id,
            token_mint,
            token_program_id,
            path,
            dead_letter_dir,
            max_retries,
            compute_unit_price,
        )
        .await
        {
            error!(path = %path.display(), error = %e, "Failed to process event");
        }
    }
}

// ---------------------------------------------------------------------------
// Single-event processing with file locking (REL-H6)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn process_event(
    client: &RpcClient,
    keypair: &Keypair,
    signing_key: &ed25519_dalek::SigningKey,
    program_id: &Pubkey,
    token_mint: &Pubkey,
    token_program_id: &Pubkey,
    path: &Path,
    dead_letter_dir: &str,
    max_retries: u32,
    compute_unit_price: u64,
) -> Result<()> {
    // File locking: rename to .processing
    let processing_path = PathBuf::from(format!("{}.processing", path.display()));
    std::fs::rename(path, &processing_path)
        .map_err(|e| anyhow!("Failed to acquire file lock (rename to .processing): {}", e))?;

    let content = std::fs::read_to_string(&processing_path)?;
    let mut queued: QueuedEvent = serde_json::from_str(&content)?;

    if queued.retries >= max_retries {
        info!(
            nonce = queued.event.nonce,
            retries = queued.retries,
            "Retry limit reached, moving to dead-letter queue"
        );
        move_to_dead_letter(&processing_path, dead_letter_dir);
        return Ok(());
    }

    info!(
        nonce = queued.event.nonce,
        retries = queued.retries,
        amount = queued.event.amount,
        "Processing event"
    );

    // Convert BridgeEvent → CompactStakeEventData for on-chain submission
    let compact = CompactStakeEventData::from(&queued.event);

    match submit_to_svm(
        client,
        keypair,
        signing_key,
        program_id,
        token_mint,
        token_program_id,
        &queued,
        &compact,
        compute_unit_price,
    )
    .await
    {
        Ok(tx_sig) => {
            info!(
                nonce = queued.event.nonce,
                tx = %tx_sig,
                "Transaction confirmed, removing from queue"
            );
            let _ = std::fs::remove_file(&processing_path);
            Ok(())
        }
        Err(e) => {
            error!(nonce = queued.event.nonce, error = %e, "SVM submission failed");

            queued.retries += 1;
            queued.last_retry_at = Some(now_secs());

            if queued.retries >= max_retries {
                warn!(
                    nonce = queued.event.nonce,
                    "Exhausted all retries, moving to dead-letter queue"
                );
                let updated = serde_json::to_string_pretty(&queued)?;
                std::fs::write(&processing_path, updated)?;
                move_to_dead_letter(&processing_path, dead_letter_dir);
            } else {
                // Exponential backoff: 2^retry seconds, capped at 64 s
                let delay = Duration::from_secs(2u64.saturating_pow(queued.retries.min(6)));
                info!(
                    nonce = queued.event.nonce,
                    next_retry = queued.retries,
                    delay_secs = delay.as_secs(),
                    "Backing off before next retry"
                );
                tokio::time::sleep(delay).await;

                let updated = serde_json::to_string_pretty(&queued)?;
                std::fs::write(&processing_path, &updated)?;
                std::fs::rename(&processing_path, path)?;
            }
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// SVM transaction submission (REL-M1 async client, REL-M2 priority fees)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn submit_to_svm(
    client: &RpcClient,
    keypair: &Keypair,
    signing_key: &ed25519_dalek::SigningKey,
    program_id: &Pubkey,
    token_mint: &Pubkey,
    token_program_id: &Pubkey,
    _queued: &QueuedEvent,
    compact: &CompactStakeEventData,
    compute_unit_price: u64,
) -> Result<String> {
    // 1. Borsh-serialize the compact event for signing
    let message = crypto::serialize_event_borsh(compact);

    // 2. Ed25519 sign the serialized message
    let sig_bytes = crypto::sign_ed25519(&message, signing_key);
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| anyhow!("Ed25519 signature must be 64 bytes"))?;

    info!(
        nonce = compact.nonce,
        message_len = message.len(),
        signature_prefix = %hex::encode(&sig_array[..8]),
        "Signed compact event with Ed25519"
    );

    // 3. Derive PDA accounts
    let relayer_pubkey = keypair.pubkey();
    let (receiver_state, _) = Pubkey::find_program_address(&[b"receiver_state"], program_id);
    let (cross_chain_request, _) = Pubkey::find_program_address(
        &[b"cross_chain_request", &compact.nonce.to_le_bytes()],
        program_id,
    );
    let (vault, _) = Pubkey::find_program_address(&[b"vault"], program_id);

    let receiver_pubkey = Pubkey::new_from_array(compact.receiver_address);
    let vault_token_account = get_associated_token_address(&vault, token_mint, token_program_id);
    let receiver_token_account =
        get_associated_token_address(&receiver_pubkey, token_mint, token_program_id);

    // 4. Build instructions

    // (a) ComputeBudget: set unit limit + priority fee (REL-M2)
    let cu_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNIT_LIMIT);
    let cu_price_ix = ComputeBudgetInstruction::set_compute_unit_price(compute_unit_price);

    // (b) Create receiver ATA if it doesn't exist (idempotent)
    let create_ata_ix = create_ata_idempotent_ix(
        &relayer_pubkey,
        &receiver_pubkey,
        token_mint,
        token_program_id,
    );

    // (c) Ed25519 signature verification instruction
    let pubkey_bytes = signing_key.verifying_key().to_bytes();
    let ed25519_ix = build_ed25519_instruction(&pubkey_bytes, &message, &sig_array);

    // (d) submit_signature program instruction
    let submit_ix = build_submit_signature_instruction(
        program_id,
        &relayer_pubkey,
        compact,
        &sig_array,
        compact.nonce,
        receiver_state,
        cross_chain_request,
        vault,
        *token_mint,
        vault_token_account,
        receiver_token_account,
        *token_program_id,
    )?;

    // 5. Build and sign transaction
    let recent_blockhash = client
        .get_latest_blockhash()
        .await
        .map_err(|e| anyhow!("Failed to get latest blockhash: {}", e))?;

    let instructions = vec![
        cu_limit_ix,
        cu_price_ix,
        create_ata_ix,
        ed25519_ix,
        submit_ix,
    ];
    let mut tx = Transaction::new_with_payer(&instructions, Some(&relayer_pubkey));
    tx.sign(&[keypair], recent_blockhash);

    info!(
        nonce = compact.nonce,
        program_id = %program_id,
        receiver_state = %receiver_state,
        cross_chain_request = %cross_chain_request,
        "Sending SVM transaction"
    );

    // 6. Simulate first to get actionable errors
    match client.simulate_transaction(&tx).await {
        Ok(sim) => {
            if let Some(err) = sim.value.err {
                let log_text = sim
                    .value
                    .logs
                    .as_ref()
                    .map(|l| l.join("\n"))
                    .unwrap_or_default();
                return Err(anyhow!("Simulation failed: {:?}\nLogs:\n{}", err, log_text));
            }
        }
        Err(e) => warn!(error = %e, "Simulation RPC error, proceeding with send"),
    }

    // 7. Send and confirm
    let sig = client
        .send_and_confirm_transaction(&tx)
        .await
        .map_err(|e| anyhow!("Failed to send transaction: {}", e))?;

    Ok(sig.to_string())
}

// ---------------------------------------------------------------------------
// Ed25519Program instruction (all offsets within same instruction → 0xFFFF)
// ---------------------------------------------------------------------------

fn build_ed25519_instruction(
    pubkey_bytes: &[u8; 32],
    message: &[u8],
    signature: &[u8; 64],
) -> Instruction {
    const DATA_START: usize = 16; // 2 bytes header + 14 bytes offsets struct
    const PUBKEY_SIZE: usize = 32;
    const SIGNATURE_SIZE: usize = 64;

    let public_key_offset = DATA_START;
    let signature_offset = public_key_offset + PUBKEY_SIZE;
    let message_data_offset = signature_offset + SIGNATURE_SIZE;

    let mut data = Vec::with_capacity(DATA_START + PUBKEY_SIZE + SIGNATURE_SIZE + message.len());

    // Header
    data.push(1u8); // num_signatures
    data.push(0u8); // padding

    // Ed25519SignatureOffsets (14 bytes)
    data.extend_from_slice(&(signature_offset as u16).to_le_bytes());
    data.extend_from_slice(&u16::MAX.to_le_bytes()); // signature_instruction_index = 0xFFFF
    data.extend_from_slice(&(public_key_offset as u16).to_le_bytes());
    data.extend_from_slice(&u16::MAX.to_le_bytes()); // public_key_instruction_index = 0xFFFF
    data.extend_from_slice(&(message_data_offset as u16).to_le_bytes());
    data.extend_from_slice(&(message.len() as u16).to_le_bytes());
    data.extend_from_slice(&u16::MAX.to_le_bytes()); // message_instruction_index = 0xFFFF

    // Payload: pubkey ‖ signature ‖ message
    data.extend_from_slice(pubkey_bytes);
    data.extend_from_slice(signature);
    data.extend_from_slice(message);

    Instruction {
        program_id: solana_sdk::ed25519_program::id(),
        accounts: vec![],
        data,
    }
}

// ---------------------------------------------------------------------------
// Anchor submit_signature instruction (REL-C3 computed discriminator)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn build_submit_signature_instruction(
    program_id: &Pubkey,
    relayer: &Pubkey,
    compact: &CompactStakeEventData,
    signature: &[u8],
    nonce: u64,
    receiver_state: Pubkey,
    cross_chain_request: Pubkey,
    vault: Pubkey,
    token_mint: Pubkey,
    vault_token_account: Pubkey,
    receiver_token_account: Pubkey,
    token_program_id: Pubkey,
) -> Result<Instruction> {
    // Anchor discriminator: sha256("global:submit_signature")[..8]  (REL-C3)
    let discriminator = anchor_discriminator("submit_signature");

    let mut data = Vec::new();
    data.extend_from_slice(&discriminator);

    // Borsh-serialized arguments matching the program's instruction layout:
    //   nonce: u64, event_data: CompactStakeEventData, signature: Vec<u8>
    nonce.serialize(&mut data)?;
    compact.serialize(&mut data)?;
    signature.to_vec().serialize(&mut data)?;

    let accounts = vec![
        AccountMeta::new(receiver_state, false),
        AccountMeta::new(cross_chain_request, false),
        AccountMeta::new(*relayer, true),
        AccountMeta::new(vault, false),
        AccountMeta::new_readonly(token_mint, false),
        AccountMeta::new(vault_token_account, false),
        AccountMeta::new(receiver_token_account, false),
        AccountMeta::new_readonly(solana_sdk::sysvar::instructions::ID, false),
        AccountMeta::new_readonly(token_program_id, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ];

    Ok(Instruction {
        program_id: *program_id,
        accounts,
        data,
    })
}

/// Compute the 8-byte Anchor instruction discriminator as
/// `sha256("global:<fn_name>")[..8]`.
fn anchor_discriminator(fn_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(format!("global:{}", fn_name).as_bytes());
    let hash = hasher.finalize();
    hash[..8].try_into().unwrap()
}

// ---------------------------------------------------------------------------
// Associated Token Account helpers (no external spl crate needed)
// ---------------------------------------------------------------------------

fn ata_program_id() -> Pubkey {
    Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
}

fn get_associated_token_address(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ata_program_id(),
    )
    .0
}

/// Build a CreateIdempotent ATA instruction without the spl crate.
fn create_ata_idempotent_ix(
    payer: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    let ata = get_associated_token_address(wallet, mint, token_program);
    Instruction {
        program_id: ata_program_id(),
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(*token_program, false),
        ],
        data: vec![1], // CreateIdempotent discriminator
    }
}

// ---------------------------------------------------------------------------
// Keypair parsing (base58 or JSON byte-array)
// ---------------------------------------------------------------------------

fn parse_keypair(s: &str) -> Result<Keypair> {
    let s = s.trim();

    // JSON array format: [12, 34, 56, ...]
    if s.starts_with('[') {
        let bytes: Vec<u8> = serde_json::from_str(s)
            .map_err(|e| anyhow!("Failed to parse JSON keypair array: {}", e))?;
        return Keypair::try_from(bytes.as_slice())
            .map_err(|e| anyhow!("Invalid keypair from JSON bytes: {}", e));
    }

    // Base58-encoded 64 bytes (Solana CLI format)
    let bytes = bs58::decode(s)
        .into_vec()
        .map_err(|e| anyhow!("Failed to decode base58 keypair: {}", e))?;
    Keypair::try_from(bytes.as_slice())
        .map_err(|e| anyhow!("Invalid keypair from base58 bytes: {}", e))
}

// ---------------------------------------------------------------------------
// Dead-letter queue
// ---------------------------------------------------------------------------

fn move_to_dead_letter(processing_path: &Path, dead_letter_dir: &str) {
    let filename = processing_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown.json.processing");
    let original_name = filename.strip_suffix(".processing").unwrap_or(filename);
    let dest = Path::new(dead_letter_dir).join(original_name);

    if let Err(e) = std::fs::rename(processing_path, &dest) {
        error!(error = %e, "Failed to move to dead-letter (rename), trying copy+delete");
        if let Ok(content) = std::fs::read(processing_path) {
            let _ = std::fs::write(&dest, content);
            let _ = std::fs::remove_file(processing_path);
        }
    } else {
        info!(dest = %dest.display(), "Moved event to dead-letter queue");
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
