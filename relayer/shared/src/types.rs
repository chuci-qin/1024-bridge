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

/// 精简的跨链事件数据（用于 SVM 提交）
/// 
/// 优化后的结构体，仅包含必需字段：
/// - 移除 source_contract, target_contract (从 receiver_state 获取)
/// - 移除 source_chain_id, target_chain_id (从 receiver_state 获取)
/// - sender 使用原始 20 字节格式
/// - receiver_address 使用 Pubkey (32 字节)
/// 
/// 总大小：76 bytes（相比原来 308 bytes，节省 75%）
#[derive(Debug, Clone)]
pub struct CompactStakeEventData {
    pub nonce: u64,                    // 8 bytes
    pub amount: u64,                   // 8 bytes
    pub block_height: u64,             // 8 bytes
    pub sender: [u8; 20],              // 20 bytes - EVM 地址原始格式
    pub receiver_pubkey: [u8; 32],     // 32 bytes - Solana Pubkey 原始格式
}

impl StakeEventData {
    /// 转换为精简格式用于 SVM 提交
    pub fn to_compact(&self) -> Result<CompactStakeEventData, String> {
        // 解析 sender (EVM 地址: 0x + 40 hex)
        let sender_bytes = if self.sender.starts_with("0x") {
            hex::decode(&self.sender[2..])
                .map_err(|e| format!("Invalid sender address: {}", e))?
        } else {
            hex::decode(&self.sender)
                .map_err(|e| format!("Invalid sender address: {}", e))?
        };
        
        if sender_bytes.len() != 20 {
            return Err(format!("Invalid sender length: expected 20 bytes, got {}", sender_bytes.len()));
        }
        
        let mut sender = [0u8; 20];
        sender.copy_from_slice(&sender_bytes);
        
        // 解析 receiver_address (Solana Base58)
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

