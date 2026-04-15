use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// Which VM family a chain belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChainKind {
    Evm,
    Svm,
}

/// Direction of a relayer task relative to the 1024 chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    /// Peer chain → 1024 chain
    Inbound,
    /// 1024 chain → peer chain
    Outbound,
}

/// Unified stake event data matching the on-chain struct on both EVM and SVM.
/// All address fields are 32 bytes; EVM addresses are right-padded to 32B.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct StakeEventData {
    pub source_contract: [u8; 32],
    pub target_contract: [u8; 32],
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub block_height: u64,
    pub amount: u64,
    pub sender: [u8; 32],
    pub receiver: [u8; 32],
    pub nonce: u64,
}

impl StakeEventData {
    /// Borsh-serialized size: 4*32 + 5*8 = 168 bytes.
    pub const BORSH_LEN: usize = 32 * 4 + 8 * 5;
}

/// Information about a discovered peer chain, read from on-chain PeerConfig.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub chain_id: u64,
    pub peer_contract: [u8; 32],
    pub kind: ChainKind,
    pub rpc_url: String,
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for ChainKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainKind::Evm => write!(f, "EVM"),
            ChainKind::Svm => write!(f, "SVM"),
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Direction::Inbound => write!(f, "inbound"),
            Direction::Outbound => write!(f, "outbound"),
        }
    }
}

impl fmt::Display for StakeEventData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "StakeEvent(nonce={}, amount={}, src_chain={}, dst_chain={})",
            self.nonce, self.amount, self.source_chain_id, self.target_chain_id,
        )
    }
}
