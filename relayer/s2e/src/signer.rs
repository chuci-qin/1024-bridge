use anyhow::{anyhow, Result};
use secp256k1::{Message, Secp256k1, SecretKey};
use shared::types::StakeEventData;
use sha2::Sha256;

/// ECDSA 签名器 (用于 EVM)
pub struct EcdsaSigner {
    secret_key: SecretKey,
}

impl EcdsaSigner {
    /// 创建新的签名器（从十六进制私钥）
    pub fn new(private_key_hex: &str) -> Result<Self> {
        // 移除可能的 0x 前缀并去除空白
        let mut private_key_hex = private_key_hex.strip_prefix("0x").unwrap_or(private_key_hex).trim().to_string();
        
        // 检查是否是示例值或占位符
        if private_key_hex.contains("Your") || private_key_hex.contains("Here") || private_key_hex.is_empty() {
            return Err(anyhow!(
                "Invalid private key: appears to be a placeholder. Please set RELAYER__ECDSA_PRIVATE_KEY to a valid 64-character hex string (32 bytes)"
            ));
        }
        
        // 处理奇数长度的十六进制字符串（在前面补0）
        if private_key_hex.len() % 2 != 0 {
            private_key_hex = format!("0{}", private_key_hex);
        }
        
        // 解析十六进制私钥
        let private_key_bytes = hex::decode(&private_key_hex)
            .map_err(|e| anyhow!("Failed to decode private key (length: {}): {}", private_key_hex.len(), e))?;

        // ECDSA secp256k1 私钥应该是 32 字节（64 个十六进制字符）
        if private_key_bytes.len() != 32 {
            return Err(anyhow!(
                "Private key must be exactly 32 bytes (64 hex characters), got {} bytes ({} hex characters). Please check RELAYER__ECDSA_PRIVATE_KEY configuration",
                private_key_bytes.len(),
                private_key_hex.len()
            ));
        }

        // 创建 SecretKey
        let secret_key = SecretKey::from_slice(&private_key_bytes)
            .map_err(|e| anyhow!("Invalid secret key: {}", e))?;

        Ok(Self { secret_key })
    }

    /// 对事件数据生成签名（EVM 格式：JSON + SHA-256 + ECDSA + EIP-191）
    pub fn sign_event(&self, event: &StakeEventData) -> Result<Vec<u8>> {
        // 1. 序列化为 JSON 格式（与 EVM 合约对齐）
        let json_message = self.serialize_event_to_json(event);

        // 2. 计算 SHA256 哈希
        let mut hasher = Sha256::new();
        hasher.update(json_message.as_bytes());
        let hash = hasher.finalize();

        // 3. 应用 EIP-191 前缀
        let prefixed = format!("\x19Ethereum Signed Message:\n32");
        let mut eth_hasher = sha3::Keccak256::new();
        use sha3::Digest as Sha3Digest;
        eth_hasher.update(prefixed.as_bytes());
        eth_hasher.update(&hash);
        let eth_hash = eth_hasher.finalize();

        // 4. 使用 secp256k1 ECDSA 签名
        let secp = Secp256k1::new();
        let message = Message::from_digest_slice(&eth_hash)
            .map_err(|e| anyhow!("Failed to create message: {}", e))?;
        let signature = secp.sign_ecdsa_recoverable(&message, &self.secret_key);

        // 5. 将签名转换为 EVM 格式 (65 字节：r + s + v)
        let (recovery_id, sig_bytes) = signature.serialize_compact();
        let mut result = Vec::with_capacity(65);
        result.extend_from_slice(&sig_bytes);
        result.push(recovery_id.to_i32() as u8 + 27); // v = recovery_id + 27
        
        Ok(result)
    }

    /// 序列化事件数据为 JSON 格式（与 EVM 合约对齐）
    fn serialize_event_to_json(&self, event: &StakeEventData) -> String {
        // Convert contract addresses to bytes32 hex format (lowercase, no 0x prefix)
        let source_contract_hex = self.contract_to_hex(&event.source_contract);
        let target_contract_hex = self.contract_to_hex(&event.target_contract);
        
        // Convert sender address to hex format (lowercase, no 0x prefix)
        let sender_hex = self.address_to_hex(&event.sender);
        
        format!(
            r#"{{"sourceContract":"{}","targetContract":"{}","chainId":"{}","blockHeight":"{}","amount":"{}","sender":"{}","receiverAddress":"{}","nonce":"{}"}}"#,
            source_contract_hex,
            target_contract_hex,
            event.source_chain_id,
            event.block_height,
            event.amount,
            sender_hex,
            event.receiver_address,
            event.nonce
        )
    }
    
    /// 转换合约地址为 bytes32 hex 格式（小写，无 0x 前缀）
    fn contract_to_hex(&self, address: &str) -> String {
        // Remove 0x prefix if present
        let addr = address.strip_prefix("0x").unwrap_or(address);
        
        // If it's already hex format
        if addr.len() == 64 && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            return addr.to_lowercase();
        }
        
        // If it's Solana base58 format, decode it
        if let Ok(decoded) = bs58::decode(addr).into_vec() {
            if decoded.len() == 32 {
                return hex::encode(&decoded);
            }
        }
        
        // Fallback: pad hex to 64 chars
        let hex_str = if addr.chars().all(|c| c.is_ascii_hexdigit()) {
            addr.to_string()
        } else {
            // Try to decode as hex anyway
            return addr.to_lowercase();
        };
        
        format!("{:0<64}", hex_str.to_lowercase())
    }
    
    /// 转换地址为 hex 格式（小写，无 0x 前缀，40 个字符）
    fn address_to_hex(&self, address: &str) -> String {
        // Remove 0x prefix if present
        let addr = address.strip_prefix("0x").unwrap_or(address);
        
        // If it's already 40-char hex format (EVM address)
        if addr.len() == 40 && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            return addr.to_lowercase();
        }
        
        // If it's 64-char hex format, take the last 40 chars (EVM address from bytes32)
        if addr.len() == 64 && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            return addr[24..].to_lowercase();
        }
        
        // If it's Solana base58 format, decode and take last 20 bytes
        if let Ok(decoded) = bs58::decode(addr).into_vec() {
            if decoded.len() >= 20 {
                let address_bytes = &decoded[decoded.len() - 20..];
                return hex::encode(address_bytes);
            }
        }
        
        // Fallback: pad or truncate to 40 chars
        let hex_str = addr.to_lowercase();
        if hex_str.len() >= 40 {
            hex_str[hex_str.len() - 40..].to_string()
        } else {
            format!("{:0>40}", hex_str)
        }
    }
}

