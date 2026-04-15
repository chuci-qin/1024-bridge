use anyhow::{Context, Result};
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, TransactionRequest, TxHash};
use tracing::info;

use crate::types::StakeEventData;

/// 4-byte selector for `confirmEvent((bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64))`
fn confirm_event_selector() -> [u8; 4] {
    let sig = "confirmEvent((bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64))";
    let hash = ethers::utils::keccak256(sig.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// 4-byte selector for `nonceConfirmations(uint64)`
fn nonce_confirmations_selector() -> [u8; 4] {
    let sig = "nonceConfirmations(uint64)";
    let hash = ethers::utils::keccak256(sig.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// ABI-encode the StakeEventData as a tuple for the confirmEvent call.
fn encode_confirm_event(event: &StakeEventData) -> Vec<u8> {
    let mut calldata = Vec::with_capacity(4 + 9 * 32);
    calldata.extend_from_slice(&confirm_event_selector());

    calldata.extend_from_slice(&event.source_contract);
    calldata.extend_from_slice(&event.target_contract);

    let mut word = [0u8; 32];
    word[24..32].copy_from_slice(&event.source_chain_id.to_be_bytes());
    calldata.extend_from_slice(&word);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.target_chain_id.to_be_bytes());
    calldata.extend_from_slice(&word);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.block_height.to_be_bytes());
    calldata.extend_from_slice(&word);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.amount.to_be_bytes());
    calldata.extend_from_slice(&word);

    calldata.extend_from_slice(&event.sender);
    calldata.extend_from_slice(&event.receiver);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.nonce.to_be_bytes());
    calldata.extend_from_slice(&word);

    calldata
}

/// Check if a nonce is already processed on EVM by calling `nonceConfirmations(nonce)`.
pub async fn check_nonce_processed(
    provider: &Provider<Http>,
    contract: Address,
    nonce: u64,
) -> Result<bool> {
    let mut calldata = Vec::with_capacity(4 + 32);
    calldata.extend_from_slice(&nonce_confirmations_selector());
    let mut word = [0u8; 32];
    word[24..32].copy_from_slice(&nonce.to_be_bytes());
    calldata.extend_from_slice(&word);

    let tx = TypedTransaction::Legacy(TransactionRequest::new().to(contract).data(calldata));

    let result = provider.call(&tx, None).await.context("call nonceConfirmations")?;

    if result.len() < 32 {
        anyhow::bail!("nonceConfirmations response too short: {} bytes", result.len());
    }

    // isProcessed is the first bool in the returned tuple
    let is_processed = result[31] != 0;
    Ok(is_processed)
}

/// Submit confirmEvent transaction to EVM chain.
pub async fn submit_confirm_event(
    wallet: &LocalWallet,
    provider: &Provider<Http>,
    contract: Address,
    chain_id: u64,
    event: &StakeEventData,
) -> Result<TxHash> {
    let wallet = wallet.clone().with_chain_id(chain_id);
    let client = SignerMiddleware::new(provider.clone(), wallet);

    let calldata = encode_confirm_event(event);

    let tx = TransactionRequest::new()
        .to(contract)
        .data(calldata);

    let pending = client
        .send_transaction(tx, None)
        .await
        .context("send confirmEvent tx")?;

    let tx_hash = pending.tx_hash();
    info!(
        nonce = event.nonce,
        tx_hash = ?tx_hash,
        "Submitted EVM confirmEvent"
    );

    Ok(tx_hash)
}
