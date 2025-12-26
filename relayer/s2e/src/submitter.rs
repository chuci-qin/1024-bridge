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
use std::sync::Arc;
use tracing::{info, warn};

/// EVM 交易提交器
pub struct EvmSubmitter {
    client: Arc<SignerMiddleware<Provider<Http>, LocalWallet>>,
    contract_address: Address,
}

impl EvmSubmitter {
    pub fn new(rpc_url: &str, contract_address: &str, private_key_hex: &str) -> Result<Self> {
        // 创建 Provider
        let provider = Provider::<Http>::try_from(rpc_url)
            .map_err(|e| anyhow!("Failed to create provider: {}", e))?;

        // 解析私钥
        let private_key_hex = private_key_hex.strip_prefix("0x").unwrap_or(private_key_hex);
        let wallet: LocalWallet = private_key_hex
            .parse()
            .map_err(|e| anyhow!("Failed to parse wallet: {}", e))?;

        // 设置链 ID (Arbitrum Sepolia)
        let wallet = wallet.with_chain_id(421614u64);

        // 创建签名中间件
        let client = Arc::new(SignerMiddleware::new(provider, wallet));

        // 解析合约地址
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

    /// 提交签名到 EVM 合约
    pub async fn submit_signature(
        &self,
        event: &StakeEventData,
        signature: &[u8],
    ) -> Result<String> {
        info!(nonce = event.nonce, "Submitting signature to EVM");

        // 构建合约调用数据
        let call_data = self.encode_submit_signature(event, signature)?;

        // 创建交易
        let tx = TransactionRequest::new()
            .to(self.contract_address)
            .data(call_data);

        // 发送交易
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

    /// 编码 submitSignature 函数调用
    fn encode_submit_signature(&self, event: &StakeEventData, signature: &[u8]) -> Result<Bytes> {
        // submitSignature 函数签名
        // function submitSignature((bytes32,bytes32,uint64,uint64,uint64,uint64,address,string,uint64) eventData, bytes signature)
        let function_signature = "submitSignature((bytes32,bytes32,uint64,uint64,uint64,uint64,address,string,uint64),bytes)";
        let selector = &ethers::utils::keccak256(function_signature.as_bytes())[0..4];

        // 解析 sender 地址
        let sender_address = self.parse_address(&event.sender)?;

        // 编码事件数据元组
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

        // 编码签名
        let signature_token = Token::Bytes(signature.to_vec());

        // 编码所有参数
        let encoded_params = encode(&[event_data_tuple, signature_token]);

        // 组合选择器和参数
        let mut call_data = Vec::with_capacity(4 + encoded_params.len());
        call_data.extend_from_slice(selector);
        call_data.extend_from_slice(&encoded_params);

        Ok(Bytes::from(call_data))
    }

    /// 解析字符串为 bytes32
    /// 支持 hex 格式（0x...）和 Solana base58 格式
    fn parse_bytes32(&self, s: &str) -> Result<[u8; 32]> {
        // 如果是 hex 格式
        if let Some(hex_str) = s.strip_prefix("0x") {
            let bytes = hex::decode(hex_str)?;
            
            if bytes.len() > 32 {
                return Err(anyhow!("Bytes too long: {} > 32", bytes.len()));
            }
            
            let mut result = [0u8; 32];
            result[..bytes.len()].copy_from_slice(&bytes);
            return Ok(result);
        }
        
        // 尝试解析为 Solana base58 格式（Pubkey）
        match bs58::decode(s).into_vec() {
            Ok(bytes) if bytes.len() == 32 => {
                let mut result = [0u8; 32];
                result.copy_from_slice(&bytes);
                Ok(result)
            }
            Ok(bytes) => Err(anyhow!("Invalid Solana pubkey length: {}", bytes.len())),
            Err(_) => {
                // 最后尝试作为hex解析（无0x前缀）
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

    /// 解析字符串为 EVM 地址（Address）
    /// 支持 hex 格式（0x...）、40字符hex、Solana base58 格式
    fn parse_address(&self, s: &str) -> Result<ethers::types::Address> {
        use ethers::types::Address;
        
        // 移除 0x 前缀（如果有）
        let s = s.strip_prefix("0x").unwrap_or(s);
        
        // 如果是 40 字符的 hex 字符串（标准 EVM 地址）
        if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            let bytes = hex::decode(s)?;
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&bytes);
            return Ok(Address::from(addr_bytes));
        }
        
        // 如果是 64 字符的 hex 字符串（bytes32 格式，取最后 20 字节）
        if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            let bytes = hex::decode(s)?;
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&bytes[12..32]);
            return Ok(Address::from(addr_bytes));
        }
        
        // 尝试解析为 Solana base58 格式（取最后 20 字节）
        if let Ok(bytes) = bs58::decode(s).into_vec() {
            if bytes.len() >= 20 {
                let mut addr_bytes = [0u8; 20];
                addr_bytes.copy_from_slice(&bytes[bytes.len() - 20..]);
                return Ok(Address::from(addr_bytes));
            }
        }
        
        // 最后尝试作为 hex 解析（支持较短的地址，前补 0）
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
