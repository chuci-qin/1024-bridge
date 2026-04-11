use crate::config::S2ESubmitterConfig;
use crate::signer::EcdsaSigner;
use anyhow::{anyhow, Result};
use ethers::{
    core::types::Address,
    middleware::SignerMiddleware,
    prelude::*,
    providers::{Http, Middleware, Provider},
    signers::{LocalWallet, Signer as EthersSigner},
    abi::{Token, encode},
};
use shared::types::StakeEventData;
use std::path::Path;
use std::sync::Arc;
use tracing::{error, info, warn};

pub struct EvmSubmitter {
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    contract_address: Address,
}

impl EvmSubmitter {
    pub fn new(rpc_url: &str, contract_address: &str, private_key_hex: &str, chain_id: u64) -> Result<Self> {
        let provider = Provider::<Http>::try_from(rpc_url)
            .map_err(|e| anyhow!("Failed to create provider: {}", e))?;

        let private_key_hex = private_key_hex.strip_prefix("0x").unwrap_or(private_key_hex);
        let wallet: LocalWallet = private_key_hex
            .parse()
            .map_err(|e| anyhow!("Failed to parse wallet: {}", e))?;
        let wallet = wallet.with_chain_id(chain_id);
        let client = Arc::new(SignerMiddleware::new(provider, wallet));

        let contract_address: Address = contract_address
            .parse()
            .map_err(|e| anyhow!("Invalid contract address: {}", e))?;

        info!(
            relayer_address = %client.address(),
            contract_address = %contract_address,
            "EVM submitter initialized"
        );

        Ok(Self {
            client,
            contract_address,
        })
    }

    pub async fn submit_signature(
        &self,
        event: &StakeEventData,
        signature: &[u8],
    ) -> Result<String> {
        info!(nonce = event.nonce, "Submitting signature to EVM");

        let call_data = self.encode_submit_signature(event, signature)?;

        let gas_price = self.client.get_gas_price().await
            .unwrap_or(ethers::types::U256::from(30_000_000_000u64));
        let buffered_gas_price = gas_price * 150 / 100;

        let tx = TransactionRequest::new()
            .to(self.contract_address)
            .data(call_data)
            .gas_price(buffered_gas_price);

        match self.client.send_transaction(tx, None).await {
            Ok(pending_tx) => {
                info!(nonce = event.nonce, "Transaction sent, waiting for confirmation");

                match pending_tx.await {
                    Ok(Some(receipt)) => {
                        info!(
                            nonce = event.nonce,
                            tx_hash = %receipt.transaction_hash,
                            "Transaction confirmed"
                        );
                        Ok(format!("{:?}", receipt.transaction_hash))
                    }
                    Ok(None) => {
                        warn!(nonce = event.nonce, "Transaction pending (no receipt yet)");
                        Err(anyhow!("Transaction pending"))
                    }
                    Err(e) => {
                        warn!(nonce = event.nonce, error = %e, "Transaction failed");
                        Err(anyhow!("Transaction failed: {}", e))
                    }
                }
            }
            Err(e) => {
                warn!(nonce = event.nonce, error = %e, "Failed to send transaction");
                Err(anyhow!("Failed to send transaction: {}", e))
            }
        }
    }

    fn encode_submit_signature(&self, event: &StakeEventData, signature: &[u8]) -> Result<Bytes> {
        let function_signature = "submitSignature((bytes32,bytes32,uint64,uint64,uint64,uint64,address,string,uint64),bytes)";
        let selector = &ethers::utils::keccak256(function_signature.as_bytes())[0..4];

        let sender_address = self.parse_address(&event.sender)?;

        let event_data_tuple = Token::Tuple(vec![
            Token::FixedBytes(self.parse_bytes32(&event.source_contract)?.to_vec()),
            Token::FixedBytes(self.parse_bytes32(&event.target_contract)?.to_vec()),
            Token::Uint(event.source_chain_id.into()),
            Token::Uint(event.target_chain_id.into()),
            Token::Uint(event.block_height.into()),
            Token::Uint(event.amount.into()),
            Token::Address(sender_address),
            Token::String(event.receiver_address.clone()),
            Token::Uint(event.nonce.into()),
        ]);

        let signature_token = Token::Bytes(signature.to_vec());
        let encoded_params = encode(&[event_data_tuple, signature_token]);

        let mut call_data = Vec::with_capacity(4 + encoded_params.len());
        call_data.extend_from_slice(selector);
        call_data.extend_from_slice(&encoded_params);

        Ok(Bytes::from(call_data))
    }

    fn parse_bytes32(&self, s: &str) -> Result<[u8; 32]> {
        if let Some(hex_str) = s.strip_prefix("0x") {
            let bytes = hex::decode(hex_str)?;
            if bytes.len() > 32 {
                return Err(anyhow!("Bytes too long: {} > 32", bytes.len()));
            }
            let mut result = [0u8; 32];
            result[..bytes.len()].copy_from_slice(&bytes);
            return Ok(result);
        }

        match bs58::decode(s).into_vec() {
            Ok(bytes) if bytes.len() == 32 => {
                let mut result = [0u8; 32];
                result.copy_from_slice(&bytes);
                Ok(result)
            }
            Ok(bytes) => Err(anyhow!("Invalid Solana pubkey length: {}", bytes.len())),
            Err(_) => {
                let bytes = hex::decode(s)?;
                if bytes.len() > 32 {
                    return Err(anyhow!("Bytes too long: {} > 32", bytes.len()));
                }
                let mut result = [0u8; 32];
                result[..bytes.len()].copy_from_slice(&bytes);
                Ok(result)
            }
        }
    }

    fn parse_address(&self, s: &str) -> Result<ethers::types::Address> {
        use ethers::types::Address;

        let s = s.strip_prefix("0x").unwrap_or(s);

        if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            let bytes = hex::decode(s)?;
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&bytes);
            return Ok(Address::from(addr_bytes));
        }

        if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            let bytes = hex::decode(s)?;
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&bytes[12..32]);
            return Ok(Address::from(addr_bytes));
        }

        if let Ok(bytes) = bs58::decode(s).into_vec() {
            if bytes.len() >= 20 {
                let mut addr_bytes = [0u8; 20];
                addr_bytes.copy_from_slice(&bytes[bytes.len() - 20..]);
                return Ok(Address::from(addr_bytes));
            }
        }

        if s.chars().all(|c| c.is_ascii_hexdigit()) {
            let bytes = hex::decode(s)?;
            if bytes.len() <= 20 {
                let mut addr_bytes = [0u8; 20];
                let start = 20 - bytes.len();
                addr_bytes[start..].copy_from_slice(&bytes);
                return Ok(Address::from(addr_bytes));
            }
        }

        Err(anyhow!("Invalid address format: {}", s))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ErrorCategory {
    Retryable,
    NonRetryable,
}

fn categorize_error(error_str: &str) -> ErrorCategory {
    let error_lower = error_str.to_lowercase();

    // EVM contract reverts are non-retryable (business logic rejection)
    if error_lower.contains("execution reverted")
        || error_lower.contains("revert")
        || error_lower.contains("already processed")
        || error_lower.contains("invalid nonce")
        || error_lower.contains("nonce too low")
        || error_lower.contains("replacement transaction underpriced")
    {
        return ErrorCategory::NonRetryable;
    }

    // Retryable: network issues, gas estimation, RPC timeouts
    ErrorCategory::Retryable
}

pub async fn start_processor(config: S2ESubmitterConfig) -> Result<()> {
    info!("Starting event processor");

    let private_key = config
        .relayer
        .ecdsa_private_key
        .as_deref()
        .ok_or_else(|| anyhow!("ECDSA private key not configured"))?;
    let signer = EcdsaSigner::new(private_key)?;
    let submitter = EvmSubmitter::new(
        &config.target_chain.rpc_url,
        &config.target_chain.contract_address,
        private_key,
        config.target_chain.chain_id,
    )?;

    let queue_dir = &config.queue.path;
    std::fs::create_dir_all(queue_dir)?;
    info!(queue_path = %queue_dir.display(), "Queue directory initialized");

    loop {
        match process_queue(&config.queue.path, &signer, &submitter).await {
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
    signer: &EcdsaSigner,
    submitter: &EvmSubmitter,
) -> Result<usize> {
    let mut processed = 0;

    let entries = std::fs::read_dir(queue_dir)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read event file {:?}: {}", path, e);
                continue;
            }
        };

        let event: StakeEventData = match serde_json::from_str(&content) {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to parse event file {:?}: {}", path, e);
                continue;
            }
        };

        info!(
            nonce = event.nonce,
            amount = event.amount,
            block_height = event.block_height,
            sender = %event.sender,
            receiver = %event.receiver_address,
            "Processing event from queue"
        );

        let signature = match signer.sign_event(&event) {
            Ok(s) => s,
            Err(e) => {
                error!(nonce = event.nonce, error = %e, "Failed to sign event");
                continue;
            }
        };

        match submitter.submit_signature(&event, &signature).await {
            Ok(tx_hash) => {
                info!(nonce = event.nonce, tx = tx_hash, "Event processed successfully");
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
                        warn!(
                            nonce = event.nonce,
                            error = %e,
                            "Non-retryable error, removing event file"
                        );
                        if let Err(remove_err) = std::fs::remove_file(&path) {
                            warn!("Failed to remove non-retryable event file: {}", remove_err);
                        }
                    }
                    ErrorCategory::Retryable => {
                        error!(
                            nonce = event.nonce,
                            error = %e,
                            "Retryable error, keeping file for retry"
                        );
                    }
                }
            }
        }
    }

    Ok(processed)
}
