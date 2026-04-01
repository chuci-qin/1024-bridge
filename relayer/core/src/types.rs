use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// Unified stake event with 32-byte sender (EVM addresses left-padded to 32 bytes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct StakeEventData {
    pub nonce: u64,
    pub amount: u64,
    pub block_height: u64,
    pub sender: [u8; 32],
    pub receiver_address: [u8; 32],
}

/// Compact event data for signing — matches the on-chain Borsh layout exactly.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CompactStakeEventData {
    pub nonce: u64,
    pub amount: u64,
    pub block_height: u64,
    pub sender: [u8; 32],
    pub receiver_address: [u8; 32],
}

/// Parsed bridge event from an on-chain log/instruction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeEvent {
    pub source_contract: String,
    pub target_contract: String,
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub block_height: u64,
    pub amount: u64,
    pub sender: [u8; 32],
    pub receiver_address: String,
    pub nonce: u64,
}

/// Wrapper used when persisting events to the retry queue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedEvent {
    pub bridge_id: String,
    pub event: BridgeEvent,
    pub retries: u32,
    pub max_retries: u32,
    pub created_at: u64,
    pub last_retry_at: Option<u64>,
}

/// Which VM family a chain belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChainType {
    Evm,
    Svm,
}

/// Direction of a bridge transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BridgeDirection {
    EvmToSvm,
    SvmToEvm,
    SvmToSvm,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<StakeEventData> for CompactStakeEventData {
    fn from(e: StakeEventData) -> Self {
        Self {
            nonce: e.nonce,
            amount: e.amount,
            block_height: e.block_height,
            sender: e.sender,
            receiver_address: e.receiver_address,
        }
    }
}

impl From<CompactStakeEventData> for StakeEventData {
    fn from(e: CompactStakeEventData) -> Self {
        Self {
            nonce: e.nonce,
            amount: e.amount,
            block_height: e.block_height,
            sender: e.sender,
            receiver_address: e.receiver_address,
        }
    }
}

impl From<&BridgeEvent> for CompactStakeEventData {
    fn from(e: &BridgeEvent) -> Self {
        let mut receiver = [0u8; 32];
        if let Ok(bytes) = hex::decode(e.receiver_address.trim_start_matches("0x")) {
            let start = 32usize.saturating_sub(bytes.len());
            receiver[start..].copy_from_slice(&bytes[..bytes.len().min(32)]);
        } else if let Ok(bytes) = bs58::decode(&e.receiver_address).into_vec() {
            let len = bytes.len().min(32);
            receiver[..len].copy_from_slice(&bytes[..len]);
        }
        Self {
            nonce: e.nonce,
            amount: e.amount,
            block_height: e.block_height,
            sender: e.sender,
            receiver_address: receiver,
        }
    }
}

impl QueuedEvent {
    pub fn new(bridge_id: String, event: BridgeEvent, max_retries: u32, now: u64) -> Self {
        Self {
            bridge_id,
            event,
            retries: 0,
            max_retries,
            created_at: now,
            last_retry_at: None,
        }
    }

    pub fn is_exhausted(&self) -> bool {
        self.retries >= self.max_retries
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for ChainType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainType::Evm => write!(f, "EVM"),
            ChainType::Svm => write!(f, "SVM"),
        }
    }
}

impl fmt::Display for BridgeDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeDirection::EvmToSvm => write!(f, "EVM → SVM"),
            BridgeDirection::SvmToEvm => write!(f, "SVM → EVM"),
            BridgeDirection::SvmToSvm => write!(f, "SVM → SVM"),
        }
    }
}

impl fmt::Display for StakeEventData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "StakeEvent(nonce={}, amount={}, sender={})",
            self.nonce,
            self.amount,
            hex::encode(self.sender),
        )
    }
}

impl fmt::Display for BridgeEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BridgeEvent(nonce={}, amount={}, sender={}, block={})",
            self.nonce,
            self.amount,
            hex::encode(self.sender),
            self.block_height,
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_stake_event() -> StakeEventData {
        StakeEventData {
            nonce: 1,
            amount: 1_000_000,
            block_height: 12345,
            sender: [0xAA; 32],
            receiver_address: [0xBB; 32],
        }
    }

    fn sample_bridge_event() -> BridgeEvent {
        BridgeEvent {
            source_contract: "aa".repeat(32),
            target_contract: "bb".repeat(32),
            source_chain_id: 1,
            target_chain_id: 2,
            block_height: 100,
            amount: 500,
            sender: [0x01; 32],
            receiver_address: "cc".repeat(32),
            nonce: 7,
        }
    }

    #[test]
    fn test_stake_to_compact_roundtrip() {
        let stake = sample_stake_event();
        let compact: CompactStakeEventData = stake.clone().into();
        let back: StakeEventData = compact.into();
        assert_eq!(stake, back);
    }

    #[test]
    fn test_bridge_event_to_compact() {
        let be = sample_bridge_event();
        let compact = CompactStakeEventData::from(&be);
        assert_eq!(compact.nonce, be.nonce);
        assert_eq!(compact.amount, be.amount);
        assert_eq!(compact.block_height, be.block_height);
        assert_eq!(compact.sender, be.sender);
    }

    #[test]
    fn test_queued_event_exhaustion() {
        let mut q = QueuedEvent::new("USDT".into(), sample_bridge_event(), 3, 1000);
        assert!(!q.is_exhausted());
        q.retries = 3;
        assert!(q.is_exhausted());
    }

    #[test]
    fn test_queued_event_json_roundtrip() {
        let q = QueuedEvent::new("USDT".into(), sample_bridge_event(), 5, 1000);
        let json = serde_json::to_string(&q).unwrap();
        let back: QueuedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.bridge_id, "USDT");
        assert_eq!(back.max_retries, 5);
        assert_eq!(back.retries, 0);
        assert!(back.last_retry_at.is_none());
    }

    #[test]
    fn test_chain_type_display() {
        assert_eq!(ChainType::Evm.to_string(), "EVM");
        assert_eq!(ChainType::Svm.to_string(), "SVM");
    }

    #[test]
    fn test_bridge_direction_display() {
        assert_eq!(BridgeDirection::EvmToSvm.to_string(), "EVM → SVM");
        assert_eq!(BridgeDirection::SvmToEvm.to_string(), "SVM → EVM");
        assert_eq!(BridgeDirection::SvmToSvm.to_string(), "SVM → SVM");
    }

    #[test]
    fn test_stake_event_display() {
        let e = sample_stake_event();
        let s = e.to_string();
        assert!(s.contains("nonce=1"));
        assert!(s.contains("amount=1000000"));
    }

    #[test]
    fn test_bridge_event_display() {
        let e = sample_bridge_event();
        let s = e.to_string();
        assert!(s.contains("nonce=7"));
        assert!(s.contains("block=100"));
    }

    #[test]
    fn test_borsh_roundtrip() {
        let compact = CompactStakeEventData {
            nonce: 99,
            amount: 42,
            block_height: 1000,
            sender: [0x11; 32],
            receiver_address: [0x22; 32],
        };
        let bytes = borsh::to_vec(&compact).unwrap();
        let decoded = CompactStakeEventData::try_from_slice(&bytes).unwrap();
        assert_eq!(compact, decoded);
    }

    #[test]
    fn test_serde_roundtrip_stake_event() {
        let e = sample_stake_event();
        let json = serde_json::to_string(&e).unwrap();
        let back: StakeEventData = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
