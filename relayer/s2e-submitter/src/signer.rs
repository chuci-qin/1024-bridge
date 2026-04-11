use anyhow::{anyhow, Result};
use secp256k1::{Message, Secp256k1, SecretKey};
use shared::types::StakeEventData;
use sha2::Sha256;

pub struct EcdsaSigner {
    secret_key: SecretKey,
}

impl EcdsaSigner {
    pub fn new(private_key_hex: &str) -> Result<Self> {
        let mut private_key_hex = private_key_hex.strip_prefix("0x").unwrap_or(private_key_hex).trim().to_string();

        if private_key_hex.contains("Your") || private_key_hex.contains("Here") || private_key_hex.is_empty() {
            return Err(anyhow!(
                "Invalid private key: appears to be a placeholder. Please set RELAYER__ECDSA_PRIVATE_KEY to a valid 64-character hex string (32 bytes)"
            ));
        }

        if private_key_hex.len() % 2 != 0 {
            private_key_hex = format!("0{}", private_key_hex);
        }

        let private_key_bytes = hex::decode(&private_key_hex)
            .map_err(|e| anyhow!("Failed to decode private key (length: {}): {}", private_key_hex.len(), e))?;

        if private_key_bytes.len() != 32 {
            return Err(anyhow!(
                "Private key must be exactly 32 bytes (64 hex characters), got {} bytes ({} hex characters). Please check RELAYER__ECDSA_PRIVATE_KEY configuration",
                private_key_bytes.len(),
                private_key_hex.len()
            ));
        }

        let secret_key = SecretKey::from_slice(&private_key_bytes)
            .map_err(|e| anyhow!("Invalid secret key: {}", e))?;

        Ok(Self { secret_key })
    }

    pub fn sign_event(&self, event: &StakeEventData) -> Result<Vec<u8>> {
        let json_message = self.serialize_event_to_json(event);

        let mut hasher = Sha256::new();
        hasher.update(json_message.as_bytes());
        let hash = hasher.finalize();

        let prefixed = format!("\x19Ethereum Signed Message:\n32");
        let mut eth_hasher = sha3::Keccak256::new();
        use sha3::Digest as Sha3Digest;
        eth_hasher.update(prefixed.as_bytes());
        eth_hasher.update(&hash);
        let eth_hash = eth_hasher.finalize();

        let secp = Secp256k1::new();
        let message = Message::from_digest_slice(&eth_hash)
            .map_err(|e| anyhow!("Failed to create message: {}", e))?;
        let signature = secp.sign_ecdsa_recoverable(&message, &self.secret_key);

        let (recovery_id, sig_bytes) = signature.serialize_compact();
        let mut result = Vec::with_capacity(65);
        result.extend_from_slice(&sig_bytes);
        result.push(recovery_id.to_i32() as u8 + 27);

        Ok(result)
    }

    fn serialize_event_to_json(&self, event: &StakeEventData) -> String {
        let source_contract_hex = self.contract_to_hex(&event.source_contract);
        let target_contract_hex = self.contract_to_hex(&event.target_contract);
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

    fn contract_to_hex(&self, address: &str) -> String {
        let addr = address.strip_prefix("0x").unwrap_or(address);

        if addr.len() == 64 && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            return addr.to_lowercase();
        }

        if let Ok(decoded) = bs58::decode(addr).into_vec() {
            if decoded.len() == 32 {
                return hex::encode(&decoded);
            }
        }

        let hex_str = if addr.chars().all(|c| c.is_ascii_hexdigit()) {
            addr.to_string()
        } else {
            return addr.to_lowercase();
        };

        format!("{:0<64}", hex_str.to_lowercase())
    }

    fn address_to_hex(&self, address: &str) -> String {
        let addr = address.strip_prefix("0x").unwrap_or(address);

        if addr.len() == 40 && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            return addr.to_lowercase();
        }

        if addr.len() == 64 && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            return addr[24..].to_lowercase();
        }

        if let Ok(decoded) = bs58::decode(addr).into_vec() {
            if decoded.len() >= 20 {
                let address_bytes = &decoded[decoded.len() - 20..];
                return hex::encode(address_bytes);
            }
        }

        let hex_str = addr.to_lowercase();
        if hex_str.len() >= 40 {
            hex_str[hex_str.len() - 40..].to_string()
        } else {
            format!("{:0>40}", hex_str)
        }
    }
}
