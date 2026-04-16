//! 核心数据类型定义
//!
//! 定义了 relayer 中跨模块使用的基础类型：
//! - ChainKind：区分 EVM 和 SVM 虚拟机
//! - Direction：区分入站（peer→1024）和出站（1024→peer）方向
//! - StakeEventData：跨链质押事件的标准化数据结构
//! - PeerInfo：已发现的对端链信息

use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// 链的虚拟机类型
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChainKind {
    /// 以太坊虚拟机系列（Ethereum、Arbitrum、Base 等）
    Evm,
    /// Solana 虚拟机系列（Solana、1024 Chain 等）
    Svm,
}

/// Relayer 任务方向（相对于 1024 链）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    /// 入站：peer 链 → 1024 链（在 1024 链上提交确认）
    Inbound,
    /// 出站：1024 链 → peer 链（在 peer 链上提交确认）
    Outbound,
}

/// 统一的跨链质押事件数据结构。
///
/// 与 EVM 合约的 StakeEvent 和 SVM 合约的 StakeEvent 一一对应。
/// 所有地址字段统一为 32 字节：
/// - SVM 地址天然 32 字节
/// - EVM 地址（20 字节）在 bytes32 中右对齐，前 12 字节为零填充
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct StakeEventData {
    /// 源链上桥合约的地址（发出事件的合约）
    pub source_contract: [u8; 32],
    /// 目标链上桥合约的地址（需要确认事件的合约）
    pub target_contract: [u8; 32],
    /// 源链的 chain_id
    pub source_chain_id: u64,
    /// 目标链的 chain_id
    pub target_chain_id: u64,
    /// 事件发生时的区块高度
    pub block_height: u64,
    /// 跨链转账金额（USDC 最小单位）
    pub amount: u64,
    /// 发送者地址
    pub sender: [u8; 32],
    /// 接收者地址
    pub receiver: [u8; 32],
    /// 全局唯一递增的 nonce，用于幂等性和去重
    pub nonce: u64,
}

impl StakeEventData {
    /// Borsh 序列化后的固定长度：4 个 bytes32 + 5 个 u64 = 4×32 + 5×8 = 168 字节
    pub const BORSH_LEN: usize = 32 * 4 + 8 * 5;
}

/// 从链上 PeerConfig PDA 解析出的对端链信息
#[derive(Clone, Debug)]
pub struct PeerInfo {
    /// 对端链的 chain_id
    pub chain_id: u64,
    /// 对端链上桥合约的地址（32 字节）
    pub peer_contract: [u8; 32],
    /// 对端链的虚拟机类型
    pub kind: ChainKind,
    /// 对端链的 RPC URL（已经过环境变量覆盖解析）
    pub rpc_url: String,
}

// ─── Display 实现 ──────────────────────────────────────────────────────

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
