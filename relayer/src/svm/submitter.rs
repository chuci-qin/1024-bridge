use anyhow::{Context, Result};
use borsh::BorshSerialize;
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;
use tracing::info;

use crate::types::StakeEventData;

const SPL_ASSOCIATED_TOKEN_ACCOUNT_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

/// Anchor instruction discriminator: SHA-256("global:confirm_event")[..8]
fn confirm_event_discriminator() -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update("global:confirm_event");
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

pub fn bridge_state_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"bridge_state"], program_id)
}

pub fn peer_config_pda(program_id: &Pubkey, chain_id: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"peer_config", &chain_id.to_le_bytes()],
        program_id,
    )
}

pub fn cross_chain_request_pda(
    program_id: &Pubkey,
    source_chain_id: u64,
    nonce: u64,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            b"cross_chain_request",
            &source_chain_id.to_le_bytes(),
            &nonce.to_le_bytes(),
        ],
        program_id,
    )
}

pub fn vault_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"vault"], program_id)
}

/// Check if a nonce is already processed on SVM.
pub fn check_nonce_processed(
    rpc: &RpcClient,
    program_id: &Pubkey,
    source_chain_id: u64,
    nonce: u64,
) -> Result<bool> {
    let (pda, _) = cross_chain_request_pda(program_id, source_chain_id, nonce);

    match rpc.get_account_with_commitment(&pda, CommitmentConfig::finalized())? {
        solana_client::rpc_response::Response { value: None, .. } => Ok(false),
        solana_client::rpc_response::Response {
            value: Some(account),
            ..
        } => {
            let data = &account.data;
            if data.len() < 8 + 8 + 4 {
                return Ok(false);
            }

            let mut offset = 8 + 8; // discriminator + nonce

            // Skip confirmed_relayers: Vec<Pubkey>
            let relayer_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
            offset += 4 + relayer_count * 32;

            if offset + 4 > data.len() {
                return Ok(false);
            }

            // Skip hash_votes: Vec<HashVote> where HashVote = [u8;32] + u8
            let vote_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
            offset += 4 + vote_count * 33;

            if offset + 3 > data.len() {
                return Ok(false);
            }

            // frozen_threshold(1) + is_unlocked(1) + is_processed(1)
            let is_processed = data[offset + 2] != 0;
            Ok(is_processed)
        }
    }
}

/// Build and send the confirm_event instruction to the SVM bridge program.
pub fn submit_confirm_event(
    rpc: &RpcClient,
    program_id: &Pubkey,
    relayer_keypair: &Keypair,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    event: &StakeEventData,
) -> Result<Signature> {
    let (bridge_state, _) = bridge_state_pda(program_id);
    let (peer_config, _) = peer_config_pda(program_id, event.source_chain_id);
    let (cross_chain_request, _) =
        cross_chain_request_pda(program_id, event.source_chain_id, event.nonce);
    let (vault, _) = vault_pda(program_id);

    let vault_token_account = spl_associated_token_account_address(&vault, usdc_mint, token_program_id);

    let receiver_pubkey = Pubkey::new_from_array(event.receiver);
    let receiver_token_account = spl_associated_token_account_address(&receiver_pubkey, usdc_mint, token_program_id);

    let mut ix_data = Vec::with_capacity(8 + 8 + 8 + StakeEventData::BORSH_LEN);
    ix_data.extend_from_slice(&confirm_event_discriminator());
    ix_data.extend_from_slice(&event.nonce.to_le_bytes());
    ix_data.extend_from_slice(&event.source_chain_id.to_le_bytes());
    event.serialize(&mut ix_data)?;

    let accounts = vec![
        AccountMeta::new(bridge_state, false),
        AccountMeta::new(peer_config, false),
        AccountMeta::new(cross_chain_request, false),
        AccountMeta::new(relayer_keypair.pubkey(), true),
        AccountMeta::new_readonly(vault, false),
        AccountMeta::new_readonly(*usdc_mint, false),
        AccountMeta::new(vault_token_account, false),
        AccountMeta::new(receiver_token_account, false),
        AccountMeta::new_readonly(*token_program_id, false),
        AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
    ];

    let ix = Instruction::new_with_bytes(*program_id, &ix_data, accounts);

    let recent_blockhash = rpc.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&relayer_keypair.pubkey()),
        &[relayer_keypair],
        recent_blockhash,
    );

    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .context("send confirm_event tx")?;

    info!(
        nonce = event.nonce,
        tx = %sig,
        "Submitted SVM confirm_event"
    );

    Ok(sig)
}

fn spl_associated_token_account_address(wallet: &Pubkey, mint: &Pubkey, token_program_id: &Pubkey) -> Pubkey {
    let ata_program_id: Pubkey = SPL_ASSOCIATED_TOKEN_ACCOUNT_ID.parse().unwrap();
    Pubkey::find_program_address(
        &[
            wallet.as_ref(),
            token_program_id.as_ref(),
            mint.as_ref(),
        ],
        &ata_program_id,
    )
    .0
}
