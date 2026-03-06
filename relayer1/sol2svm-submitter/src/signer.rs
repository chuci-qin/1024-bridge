use anyhow::Result;
use borsh::BorshSerialize;
use shared::types::CompactStakeEventData;
use solana_sdk::signature::{Keypair, SeedDerivable, Signer};

/// Ed25519 signer for submitting to 1024chain (same as e2s-submitter).
pub struct Ed25519Signer {
    keypair: Keypair,
}

impl Ed25519Signer {
    pub fn new(private_key_str: &str) -> Result<Self> {
        let private_key_bytes = if private_key_str.contains(',') {
            private_key_str
                .split(',')
                .map(|s| {
                    s.trim()
                        .parse::<u8>()
                        .map_err(|e| anyhow::anyhow!("Failed to parse byte: {}", e))
                })
                .collect::<Result<Vec<u8>>>()?
        } else if private_key_str.len() == 64 || private_key_str.starts_with("0x") {
            let hex_str = private_key_str.trim_start_matches("0x");
            hex::decode(hex_str)
                .map_err(|e| anyhow::anyhow!("Failed to decode hex: {}", e))?
        } else {
            bs58::decode(private_key_str)
                .into_vec()
                .map_err(|e| anyhow::anyhow!("Failed to decode base58: {}", e))?
        };

        if private_key_bytes.len() < 32 {
            anyhow::bail!(
                "Private key must be at least 32 bytes, got {}",
                private_key_bytes.len()
            );
        }

        let seed = &private_key_bytes[0..32];
        let keypair = Keypair::from_seed(seed)
            .map_err(|e| anyhow::anyhow!("Failed to create keypair from seed: {}", e))?;

        Ok(Self { keypair })
    }

    /// Sign compact event data using Ed25519 (Borsh serialized, matching 1024chain on-chain format).
    pub fn sign_compact_event(&self, event: &CompactStakeEventData) -> Result<Vec<u8>> {
        use tracing::debug;

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

        let signature = self.keypair.sign_message(&message);

        debug!(
            signature_hex = %hex::encode(signature.as_ref()),
            pubkey = %self.keypair.pubkey(),
            "Generated Ed25519 signature"
        );

        Ok(signature.as_ref().to_vec())
    }

    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }
}
