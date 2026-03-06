use serde::{Deserialize, Serialize};
use std::fmt;

/// 跨链事件数据 (统一格式)
/// 
/// 注意：此结构包含完整的事件信息用于存储和日志，
/// 但在提交到 SVM 时会转换为精简格式以满足交易大小限制
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
pub struct StakeEventData {
    // 保留完整信息用于日志和验证
    pub source_contract: String,
    pub target_contract: String,
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub block_height: u64,
    pub amount: u64,
    pub sender: String,              // EVM 发起者地址（如 0xd4B42...）
    pub receiver_address: String,    // Solana 接收地址（Base58）
    pub nonce: u64,
}

/// Compact cross-chain event data for SVM submission.
/// 
/// Unified 32-byte sender format:
/// - EVM source: first 12 bytes are 0x00, last 20 bytes are the EVM address
/// - Solana source: full 32-byte pubkey
/// 
/// Total size: 88 bytes
#[derive(Debug, Clone)]
pub struct CompactStakeEventData {
    pub nonce: u64,                    // 8 bytes
    pub amount: u64,                   // 8 bytes
    pub block_height: u64,             // 8 bytes
    pub sender: [u8; 32],              // 32 bytes - unified sender (EVM zero-padded or Solana pubkey)
    pub receiver_pubkey: [u8; 32],     // 32 bytes - 1024chain receiver Pubkey
}

impl StakeEventData {
    /// Convert to compact format for SVM submission.
    /// Supports both EVM (20-byte, 0x-prefixed hex) and Solana (32-byte, base58) sender addresses.
    /// EVM addresses are left-padded with 12 zero bytes to fill the 32-byte sender field.
    pub fn to_compact(&self) -> Result<CompactStakeEventData, String> {
        let sender_hex = if self.sender.starts_with("0x") {
            &self.sender[2..]
        } else {
            &self.sender
        };
        
        let sender_bytes = hex::decode(sender_hex)
            .map_err(|_| {
                // Not hex — try base58 (Solana pubkey)
                String::new()
            })
            .or_else(|_| {
                bs58::decode(&self.sender)
                    .into_vec()
                    .map_err(|e| format!("Invalid sender address (not hex or base58): {}", e))
            })?;
        
        let mut sender = [0u8; 32];
        match sender_bytes.len() {
            20 => {
                // EVM address: zero-pad first 12 bytes, place address in last 20
                sender[12..].copy_from_slice(&sender_bytes);
            }
            32 => {
                // Solana pubkey: use all 32 bytes directly
                sender.copy_from_slice(&sender_bytes);
            }
            other => {
                return Err(format!("Invalid sender length: expected 20 or 32 bytes, got {}", other));
            }
        }
        
        // Parse receiver_address (1024chain base58 pubkey)
        let receiver_bytes = bs58::decode(&self.receiver_address)
            .into_vec()
            .map_err(|e| format!("Invalid receiver address: {}", e))?;
        
        if receiver_bytes.len() != 32 {
            return Err(format!("Invalid receiver length: expected 32 bytes, got {}", receiver_bytes.len()));
        }
        
        let mut receiver_pubkey = [0u8; 32];
        receiver_pubkey.copy_from_slice(&receiver_bytes);
        
        Ok(CompactStakeEventData {
            nonce: self.nonce,
            amount: self.amount,
            block_height: self.block_height,
            sender,
            receiver_pubkey,
        })
    }
}

/// 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::Processing => write!(f, "processing"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
        }
    }
}

/// 任务信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub nonce: u64,
    pub status: TaskStatus,
    pub event_data: StakeEventData,
    pub signature: Option<String>,
    pub transaction_hash: Option<String>,
    pub retries: u32,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Relayer 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayerInfo {
    pub address: String,
    pub whitelisted: bool,
    pub balance_svm: Option<f64>,
    pub balance_evm: Option<f64>,
}

/// 链信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainInfo {
    pub name: String,
    pub chain_id: u64,
    pub rpc: String,
    pub connected: bool,
    pub last_block: Option<u64>,
}

/// 服务状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub service: String,
    pub listening: bool,
    pub source_chain: ChainInfo,
    pub target_chain: ChainInfo,
    pub relayer: RelayerInfo,
}

/// 队列状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStatus {
    pub pending: u64,
    pub processing: u64,
    pub completed: u64,
    pub failed: u64,
    pub tasks: Vec<TaskSummary>,
}

/// 任务摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub nonce: u64,
    pub status: TaskStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub retries: u32,
}

/// Nonce 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NonceInfo {
    pub source_chain: SourceNonceInfo,
    pub target_chain: TargetNonceInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceNonceInfo {
    pub current: u64,
    pub last_processed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetNonceInfo {
    pub last_nonce: u64,
    pub pending: Vec<u64>,
}

/// 健康检查响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub version: String,
    pub uptime: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

