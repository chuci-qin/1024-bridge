use serde::{Deserialize, Serialize};

/// Chain ID constants
pub const CHAIN_ID_EVM: u16 = 1;
pub const CHAIN_ID_SOLANA: u16 = 2;

/// Guardian network constants
pub const GUARDIAN_SET_SIZE: usize = 19;
pub const SIGNATURE_QUORUM: usize = 13;

/// VAA version
pub const VAA_VERSION: u8 = 1;

/// Message observation from a blockchain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Source chain ID
    pub emitter_chain: u16,
    
    /// Emitter address (32 bytes)
    pub emitter_address: [u8; 32],
    
    /// Message sequence number
    pub sequence: u64,
    
    /// Transaction hash
    pub tx_hash: String,
    
    /// Block number or slot
    pub block_number: u64,
    
    /// Timestamp
    pub timestamp: u32,
    
    /// Nonce
    pub nonce: u32,
    
    /// Consistency level
    pub consistency_level: u8,
    
    /// Message payload
    pub payload: Vec<u8>,
}

/// Message ID for tracking
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId {
    pub emitter_chain: u16,
    pub emitter_address: [u8; 32],
    pub sequence: u64,
}

impl From<&Observation> for MessageId {
    fn from(obs: &Observation) -> Self {
        Self {
            emitter_chain: obs.emitter_chain,
            emitter_address: obs.emitter_address,
            sequence: obs.sequence,
        }
    }
}

/// ECDSA signature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub guardian_index: u8,
    pub r: [u8; 32],
    pub s: [u8; 32],
    pub v: u8,
}

/// Signed observation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedObservation {
    pub observation: Observation,
    pub signature: Signature,
}

/// VAA (Verifiable Action Approval)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VAA {
    pub version: u8,
    pub guardian_set_index: u32,
    pub signatures: Vec<Signature>,
    pub timestamp: u32,
    pub nonce: u32,
    pub emitter_chain: u16,
    pub emitter_address: [u8; 32],
    pub sequence: u64,
    pub consistency_level: u8,
    pub payload: Vec<u8>,
}

impl VAA {
    /// Get message ID
    pub fn message_id(&self) -> MessageId {
        MessageId {
            emitter_chain: self.emitter_chain,
            emitter_address: self.emitter_address,
            sequence: self.sequence,
        }
    }
    
    /// Calculate message digest (double keccak256)
    pub fn digest(&self) -> [u8; 32] {
        use sha3::{Digest, Keccak256};
        
        let mut data = Vec::new();
        data.extend_from_slice(&self.timestamp.to_be_bytes());
        data.extend_from_slice(&self.nonce.to_be_bytes());
        data.extend_from_slice(&self.emitter_chain.to_be_bytes());
        data.extend_from_slice(&self.emitter_address);
        data.extend_from_slice(&self.sequence.to_be_bytes());
        data.push(self.consistency_level);
        data.extend_from_slice(&self.payload);
        
        // Double hash
        let hash1 = Keccak256::digest(&data);
        let hash2 = Keccak256::digest(&hash1);
        
        hash2.into()
    }
}

