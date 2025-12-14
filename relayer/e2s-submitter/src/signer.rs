use anyhow::Result;
use borsh::BorshSerialize;
use shared::types::CompactStakeEventData;
use solana_sdk::signature::{Keypair, SeedDerivable, Signer};

/// Ed25519 签名器 (用于 SVM)
pub struct Ed25519Signer {
    keypair: Keypair,
}

impl Ed25519Signer {
    /// 创建新的签名器（支持多种私钥格式）
    /// 支持的格式：
    /// - 十六进制字符串 (64个字符)
    /// - 逗号分隔的数字 (例如: "1,2,3,...")
    /// - Base58 格式
    pub fn new(private_key_str: &str) -> Result<Self> {
        let private_key_bytes = if private_key_str.contains(',') {
            // 逗号分隔的数字格式
            private_key_str
                .split(',')
                .map(|s| {
                    s.trim()
                        .parse::<u8>()
                        .map_err(|e| anyhow::anyhow!("Failed to parse byte: {}", e))
                })
                .collect::<Result<Vec<u8>>>()?
        } else if private_key_str.len() == 64 || private_key_str.starts_with("0x") {
            // 十六进制格式
            let hex_str = private_key_str.trim_start_matches("0x");
            hex::decode(hex_str)
                .map_err(|e| anyhow::anyhow!("Failed to decode hex: {}", e))?
        } else {
            // 尝试 Base58 格式
            bs58::decode(private_key_str)
                .into_vec()
                .map_err(|e| anyhow::anyhow!("Failed to decode base58: {}", e))?
        };

        // Ed25519 私钥应该是 32 或 64 字节
        if private_key_bytes.len() < 32 {
            anyhow::bail!(
                "Private key must be at least 32 bytes, got {}",
                private_key_bytes.len()
            );
        }

        // 取前 32 字节作为种子
        let seed = &private_key_bytes[0..32];

        // 使用 Solana SDK 从种子创建 Keypair
        let keypair = Keypair::from_seed(seed)
            .map_err(|e| anyhow::anyhow!("Failed to create keypair from seed: {}", e))?;

        Ok(Self { keypair })
    }


    /// 对精简格式的事件数据生成签名
    /// 
    /// 使用 Borsh 序列化精简的事件数据并签名
    /// 这与 SVM 合约期望的格式完全一致
    pub fn sign_compact_event(&self, event: &CompactStakeEventData) -> Result<Vec<u8>> {
        use tracing::debug;
        
        // 1. Borsh 序列化精简事件数据
        // 按照 SVM 合约中的字段顺序：nonce, amount, block_height, sender, receiver_address
        let mut message = Vec::new();
        event.nonce.serialize(&mut message)?;
        event.amount.serialize(&mut message)?;
        event.block_height.serialize(&mut message)?;
        event.sender.serialize(&mut message)?;
        event.receiver_pubkey.serialize(&mut message)?;

        debug!(
            message_len = message.len(),
            message_hex = %hex::encode(&message),
            "Serialized message for signing (Borsh format)"
        );

        // 2. 直接签名原始消息（与合约期望一致）
        let signature = self.keypair.sign_message(&message);

        debug!(
            signature_hex = %hex::encode(signature.as_ref()),
            pubkey = %self.keypair.pubkey(),
            "Generated Ed25519 signature"
        );

        // 3. 返回 64 字节签名
        Ok(signature.as_ref().to_vec())
    }

    /// 获取 Solana Keypair 的引用（用于交易签名）
    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }
}

