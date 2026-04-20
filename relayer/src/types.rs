//! 核心数据类型定义
//!
//! 定义了 relayer 中跨模块使用的基础类型：
//! - ChainKind：区分 EVM 和 SVM 虚拟机
//! - StakeEventData：跨链质押事件的标准化数据结构
//! - PeerInfo：已发现的对端链信息
//!
//! 注：旧的 `Direction` (Inbound/Outbound) 已废除。新架构下 1024 与 peer 完全
//! 对称，路由由 `event.target_chain_id` 直接决定，没有方向概念。

use std::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// `[u8; 32]` 地址字段的自定义 serde 序列化/反序列化。
///
/// 序列化规则（自动检测 EVM vs SVM）：
/// - 前 12 字节全零 → EVM 地址，输出 `"0x"` + 后 20 字节 hex（40 字符）
/// - 否则 → SVM 地址，输出 base58 编码
///
/// 反序列化兼容两种输入：
/// - 字符串（新格式）：`"0x..."` 解析为 EVM hex，其余尝试 base58
/// - 数组（旧格式）：`[11, 22, ...]` 兼容已有磁盘文件
mod bytes32_display {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if bytes[..12].iter().all(|&b| b == 0) && bytes[12..].iter().any(|&b| b != 0) {
            let hex_str = format!("0x{}", hex::encode(&bytes[12..]));
            serializer.serialize_str(&hex_str)
        } else {
            serializer.serialize_str(&bs58::encode(bytes).into_string())
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrArray {
            Str(String),
            Array(Vec<u8>),
        }

        match StringOrArray::deserialize(deserializer)? {
            StringOrArray::Str(s) => parse_address_str(&s).map_err(serde::de::Error::custom),
            StringOrArray::Array(v) => {
                let arr: [u8; 32] = v
                    .try_into()
                    .map_err(|v: Vec<u8>| {
                        serde::de::Error::custom(format!(
                            "expected 32 bytes, got {}",
                            v.len()
                        ))
                    })?;
                Ok(arr)
            }
        }
    }

    fn parse_address_str(s: &str) -> Result<[u8; 32], String> {
        if let Some(hex_body) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            let raw = hex::decode(hex_body).map_err(|e| format!("invalid hex: {e}"))?;
            if raw.len() == 20 {
                let mut out = [0u8; 32];
                out[12..].copy_from_slice(&raw);
                Ok(out)
            } else if raw.len() == 32 {
                let mut out = [0u8; 32];
                out.copy_from_slice(&raw);
                Ok(out)
            } else {
                Err(format!("hex address must be 20 or 32 bytes, got {}", raw.len()))
            }
        } else {
            let raw = bs58::decode(s)
                .into_vec()
                .map_err(|e| format!("invalid base58: {e}"))?;
            if raw.len() == 32 {
                let mut out = [0u8; 32];
                out.copy_from_slice(&raw);
                Ok(out)
            } else {
                Err(format!("base58 address must be 32 bytes, got {}", raw.len()))
            }
        }
    }
}

/// 链的虚拟机类型
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChainKind {
    /// 以太坊虚拟机系列（Ethereum、Arbitrum、Base 等）
    Evm,
    /// Solana 虚拟机系列（Solana、1024 Chain 等）
    Svm,
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
    #[serde(with = "bytes32_display")]
    pub source_contract: [u8; 32],
    /// 目标链上桥合约的地址（需要确认事件的合约）
    #[serde(with = "bytes32_display")]
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
    #[serde(with = "bytes32_display")]
    pub sender: [u8; 32],
    /// 接收者地址
    #[serde(with = "bytes32_display")]
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

impl fmt::Display for StakeEventData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "StakeEvent(nonce={}, amount={}, src_chain={}, dst_chain={})",
            self.nonce, self.amount, self.source_chain_id, self.target_chain_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试夹具：构造一个所有字段非平凡值的 StakeEventData。
    fn sample_event() -> StakeEventData {
        StakeEventData {
            source_contract: [0x11; 32],
            target_contract: [0x22; 32],
            source_chain_id: 91024,
            target_chain_id: 1,
            block_height: 0xdead_beef,
            amount: 1_000_000,
            sender: [0x33; 32],
            receiver: [0x44; 32],
            nonce: 42,
        }
    }

    /// 关键不变量：BORSH_LEN 必须等于实际 borsh 序列化长度。
    /// 如果有人在 StakeEventData 里加了字段但忘了更新 BORSH_LEN，
    /// SVM poller 中 `data.len() < 8 + BORSH_LEN` 的检查会出错。
    #[test]
    fn borsh_len_constant_matches_serialization() {
        let ev = sample_event();
        let bytes = borsh::to_vec(&ev).expect("borsh serialize");
        assert_eq!(bytes.len(), StakeEventData::BORSH_LEN);
        // 4 个 bytes32 + 5 个 u64 = 128 + 40 = 168
        assert_eq!(StakeEventData::BORSH_LEN, 168);
    }

    /// borsh 反序列化应能完美恢复原始数据。
    #[test]
    fn borsh_roundtrip_preserves_all_fields() {
        let original = sample_event();
        let bytes = borsh::to_vec(&original).expect("borsh serialize");
        let recovered = StakeEventData::try_from_slice(&bytes).expect("borsh deserialize");
        assert_eq!(original, recovered);
    }

    /// borsh 用小端序：验证 nonce=42 被编码为最后 8 字节 `[42, 0, 0, 0, 0, 0, 0, 0]`。
    /// 这个保证 SVM 端 (Anchor borsh) 与 relayer 端字节级兼容。
    #[test]
    fn borsh_uses_little_endian_for_u64() {
        let mut ev = sample_event();
        ev.nonce = 0x42;
        let bytes = borsh::to_vec(&ev).expect("borsh serialize");
        let nonce_bytes = &bytes[bytes.len() - 8..];
        assert_eq!(nonce_bytes, [0x42, 0, 0, 0, 0, 0, 0, 0]);
    }
}
