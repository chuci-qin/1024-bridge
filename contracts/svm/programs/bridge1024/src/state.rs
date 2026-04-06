use anchor_lang::prelude::*;

pub const MAX_RELAYERS: usize = 18;
pub const MAX_FEE: u64 = 1_000_000_000;
pub const TIMELOCK_DELAY: u64 = 24 * 3600;
pub const TIMELOCK_GRACE_PERIOD: u64 = 48 * 3600;

/// 统一桥状态 PDA（合并了原 SenderState + ReceiverState）。
/// 单个 PDA 存储所有配置、角色、速率限制和中继器列表。
/// Seeds: [b"bridge_state"]
#[account]
pub struct BridgeState {
    // ─── 角色 ───
    pub admin: Pubkey,
    pub guardian: Pubkey,
    pub operator: Pubkey,
    pub recovery: Pubkey,
    pub pending_admin: Pubkey,

    // ─── 配置 ───
    pub vault: Pubkey,
    pub usdc_mint: Pubkey,
    pub peer_contract: [u8; 32],
    pub local_chain_id: u64,
    pub peer_chain_id: u64,

    // ─── 发送端状态 ───
    pub sender_nonce: u64,

    // ─── 速率限制（滑动窗口） ───
    pub max_unlock_per_window: u64,
    pub window_duration: u64,
    pub current_window_start: u64,
    pub current_window_usage: u64,
    pub previous_window_usage: u64,
    pub max_single_unlock: u64,
    pub max_stake_amount: u64,
    pub minimum_reserve: u64,

    // ─── 手续费（SVM 特有） ───
    pub bridge_fee: u64,

    // ─── 标志位 ───
    pub is_paused: bool,
    pub timelock_active: bool,

    // ─── 中继器 ───
    pub relayer_count: u8,
    pub relayers: Vec<Pubkey>,
}

impl BridgeState {
    pub const LEN: usize = 8 // 鉴别器
        + 32 * 5  // 角色
        + 32 * 2  // vault + usdc_mint
        + 32      // peer_contract
        + 8 * 2   // 链 ID
        + 8       // sender_nonce
        + 8 * 8   // 速率限制字段
        + 8       // bridge_fee
        + 1 * 2   // 标志位
        + 1       // relayer_count
        + (4 + MAX_RELAYERS * 32); // relayers vec

    pub fn is_relayer(&self, addr: &Pubkey) -> bool {
        self.relayers.iter().any(|r| r == addr)
    }
}

/// 每个 nonce 的质押记录，用于退款。
/// Seeds: [b"stake_record", nonce.to_le_bytes()]
#[account]
pub struct StakeRecord {
    pub owner: Pubkey,
    pub amount: u64,
    pub refunded: bool,
}

impl StakeRecord {
    pub const LEN: usize = 8 + 32 + 8 + 1;
}

/// 哈希投票条目：记录有多少中继器为某个事件数据哈希投票。
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct HashVote {
    pub data_hash: [u8; 32],
    pub count: u8,
}

/// 每个 nonce 的跨链请求 PDA，追踪确认收集和哈希投票以解锁代币。
/// Seeds: [b"cross_chain_request", nonce.to_le_bytes()]
#[account]
pub struct CrossChainRequest {
    pub nonce: u64,
    pub confirmed_relayers: Vec<Pubkey>,
    pub hash_votes: Vec<HashVote>,
    pub frozen_threshold: u8,
    pub is_unlocked: bool,
}

impl CrossChainRequest {
    pub const LEN: usize = 8 // 鉴别器
        + 8                           // nonce
        + (4 + MAX_RELAYERS * 32)     // confirmed_relayers
        + (4 + MAX_RELAYERS * (32+1)) // hash_votes
        + 1                           // frozen_threshold
        + 1;                          // is_unlocked
}

/// 时间锁操作 PDA，由 schedule_operation 创建，由实际指令消费。
/// Seeds: [b"timelock", op_hash.as_ref()]
#[account]
pub struct TimelockOperation {
    pub eta: u64,
    pub op_hash: [u8; 32],
}

impl TimelockOperation {
    pub const LEN: usize = 8 + 8 + 32;
}

/// 跨链事件数据，与 EVM 的 StakeEventData 完全对齐。
/// 在 confirm_event 中用于哈希投票。
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
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
    pub const LEN: usize = 32 * 4 + 8 * 5;
}

impl Default for StakeEventData {
    fn default() -> Self {
        Self {
            source_contract: [0u8; 32],
            target_contract: [0u8; 32],
            source_chain_id: 0,
            target_chain_id: 0,
            block_height: 0,
            amount: 0,
            sender: [0u8; 32],
            receiver: [0u8; 32],
            nonce: 0,
        }
    }
}
