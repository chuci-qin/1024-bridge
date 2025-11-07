use anyhow::{Result, Context};
use secp256k1::{Secp256k1, SecretKey, Message as SecpMessage};
use sha3::{Digest, Keccak256};
use tracing::info;

use crate::types::{Observation, Signature};

pub struct Signer {
    secp: Secp256k1<secp256k1::All>,
    secret_key: SecretKey,
    guardian_index: u8,
    ethereum_address: [u8; 20],
}

impl Signer {
    /// Create a new signer with a private key
    pub fn new(private_key_hex: &str, guardian_index: u8) -> Result<Self> {
        let secp = Secp256k1::new();
        
        // Parse private key
        let private_key_bytes = hex::decode(private_key_hex.trim_start_matches("0x"))
            .context("Invalid private key hex")?;
        
        let secret_key = SecretKey::from_slice(&private_key_bytes)
            .context("Invalid secret key")?;
        
        // Derive public key and Ethereum address
        let public_key = secp256k1::PublicKey::from_secret_key(&secp, &secret_key);
        let ethereum_address = Self::pubkey_to_eth_address(&public_key);
        
        info!(
            "🔑 Signer initialized: guardian_index={}, address={:?}",
            guardian_index,
            hex::encode(ethereum_address)
        );
        
        Ok(Self {
            secp,
            secret_key,
            guardian_index,
            ethereum_address,
        })
    }
    
    /// Generate a new random signer (for testing)
    pub fn generate_random(guardian_index: u8) -> Result<Self> {
        let secp = Secp256k1::new();
        let mut rng = rand::thread_rng();
        let secret_key = SecretKey::new(&mut rng);
        
        let public_key = secp256k1::PublicKey::from_secret_key(&secp, &secret_key);
        let ethereum_address = Self::pubkey_to_eth_address(&public_key);
        
        info!(
            "🎲 Generated random signer: index={}, address={}",
            guardian_index,
            hex::encode(ethereum_address)
        );
        
        Ok(Self {
            secp,
            secret_key,
            guardian_index,
            ethereum_address,
        })
    }
    
    /// Sign an observation
    pub fn sign_observation(&self, observation: &Observation) -> Result<Signature> {
        // Calculate message digest (double keccak256)
        let digest = self.calculate_digest(observation)?;
        
        // Sign the digest
        let message = SecpMessage::from_digest(digest);
        let recoverable_sig = self.secp
            .sign_ecdsa_recoverable(&message, &self.secret_key);
        
        // Extract r, s, v
        let (recovery_id, signature_bytes) = recoverable_sig.serialize_compact();
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&signature_bytes[0..32]);
        s.copy_from_slice(&signature_bytes[32..64]);
        let v = recovery_id.to_i32() as u8 + 27;
        
        info!(
            "✍️  Signed observation: chain={}, seq={}, v={}",
            observation.emitter_chain,
            observation.sequence,
            v
        );
        
        Ok(Signature {
            guardian_index: self.guardian_index,
            r,
            s,
            v,
        })
    }
    
    /// Calculate observation digest (double keccak256)
    fn calculate_digest(&self, observation: &Observation) -> Result<[u8; 32]> {
        let mut data = Vec::new();
        
        // Serialize observation data (matching VAA body format)
        data.extend_from_slice(&observation.timestamp.to_be_bytes());
        data.extend_from_slice(&observation.nonce.to_be_bytes());
        data.extend_from_slice(&observation.emitter_chain.to_be_bytes());
        data.extend_from_slice(&observation.emitter_address);
        data.extend_from_slice(&observation.sequence.to_be_bytes());
        data.push(observation.consistency_level);
        data.extend_from_slice(&observation.payload);
        
        // Double keccak256 hash
        let hash1 = Keccak256::digest(&data);
        let hash2 = Keccak256::digest(&hash1);
        
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&hash2);
        
        Ok(digest)
    }
    
    /// Convert public key to Ethereum address
    fn pubkey_to_eth_address(public_key: &secp256k1::PublicKey) -> [u8; 20] {
        let pubkey_bytes = public_key.serialize_uncompressed();
        let hash = Keccak256::digest(&pubkey_bytes[1..]); // Skip first byte (0x04)
        let mut address = [0u8; 20];
        address.copy_from_slice(&hash[12..]); // Take last 20 bytes
        address
    }
    
    /// Get Ethereum address
    pub fn ethereum_address(&self) -> [u8; 20] {
        self.ethereum_address
    }
    
    /// Get guardian index
    pub fn index(&self) -> u8 {
        self.guardian_index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_signer_creation() {
        let signer = Signer::generate_random(0).unwrap();
        assert_eq!(signer.index(), 0);
        assert_ne!(signer.ethereum_address(), [0u8; 20]);
    }
    
    #[test]
    fn test_sign_observation() {
        let signer = Signer::generate_random(5).unwrap();
        
        let observation = Observation {
            emitter_chain: 1,
            emitter_address: [0u8; 32],
            sequence: 42,
            tx_hash: "test".to_string(),
            block_number: 100,
            timestamp: 1699264800,
            nonce: 12345,
            consistency_level: 200,
            payload: vec![1, 2, 3, 4],
        };
        
        let signature = signer.sign_observation(&observation).unwrap();
        assert_eq!(signature.guardian_index, 5);
        assert!(signature.v == 27 || signature.v == 28);
    }
}

