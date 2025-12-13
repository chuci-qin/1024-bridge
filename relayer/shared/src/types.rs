use serde::{Deserialize, Serialize};
use std::fmt;

/// 跨链事件数据 (统一格式)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
pub struct StakeEventData {
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

