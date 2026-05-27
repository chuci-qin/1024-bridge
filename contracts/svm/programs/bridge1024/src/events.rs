use anchor_lang::prelude::*;

/// 用户 stake（锁定）USDC 时触发。
/// 中继器监听此事件以发起目标链解锁。
/// 所有字段使用固定宽度类型，与 EVM 的 BridgeEventData 完全对齐。
#[event]
pub struct Staked {
    /// 源链桥合约地址（本程序 ID 的 bytes32 表示）
    pub source_contract: [u8; 32],
    /// 目标链桥合约地址
    pub target_contract: [u8; 32],
    /// 源链 ID
    pub source_chain_id: u64,
    /// 目标链 ID
    pub target_chain_id: u64,
    /// stake 发生时的 slot 高度
    pub block_height: u64,
    /// 用户实付全额（含手续费，USDC 原始精度 6 位小数）
    pub raw_amount: u64,
    /// 锁定金额（扣除手续费后的净额，USDC 原始精度 6 位小数）
    pub amount: u64,
    /// 发送者地址（Solana 原生 32 字节公钥）
    pub sender: [u8; 32],
    /// 接收者地址（EVM 右对齐 20B，SVM 原生 32B）
    pub receiver: [u8; 32],
    /// 客户端生成的随机唯一事件编号，防重放
    pub nonce: u64,
}

/// 确认达到阈值后成功解锁代币，字段布局与 Staked 完全一致
#[event]
pub struct Unlocked {
    pub source_contract: [u8; 32],
    pub target_contract: [u8; 32],
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub block_height: u64,
    pub raw_amount: u64,
    pub amount: u64,
    pub sender: [u8; 32],
    pub receiver: [u8; 32],
    pub nonce: u64,
}

/// 新中继器加入白名单
#[event]
pub struct RelayerAdded {
    pub relayer: Pubkey,
}

/// 中继器从白名单移除
#[event]
pub struct RelayerRemoved {
    pub relayer: Pubkey,
}

/// 中继器为某 nonce 提交了确认投票，data_hash 用于监控投票一致性
#[event]
pub struct EventConfirmed {
    pub relayer: Pubkey,
    pub nonce: u64,
    pub data_hash: [u8; 32],
}

/// 守护者地址变更
#[event]
pub struct GuardianUpdated {
    pub old_guardian: Pubkey,
    pub new_guardian: Pubkey,
}

/// 管理员发起两步转移，提议新管理员
#[event]
pub struct AdminTransferProposed {
    pub current_admin: Pubkey,
    pub pending_admin: Pubkey,
}

/// 新管理员接受转移，管理权正式交接
#[event]
pub struct AdminTransferAccepted {
    pub old_admin: Pubkey,
    pub new_admin: Pubkey,
}

/// 桥核心参数变更（与 EVM `BridgeConfigured` 对齐：(usdc, peer, localId, peerId, fee)）
#[event]
pub struct BridgeConfigured {
    pub usdc_mint: Pubkey,
    pub peer_contract: [u8; 32],
    pub local_chain_id: u64,
    pub peer_chain_id: u64,
    pub bridge_fee: u64,
}

/// 速率限制参数变更（leaf 版本：仅一层）
#[event]
pub struct RateLimitsConfigured {
    pub max_unlock_per_window: u64,
    pub window_duration: u64,
    pub max_single_unlock: u64,
    pub max_stake_amount: u64,
    pub minimum_reserve: u64,
}

/// 桥手续费独立变更（与 EVM `BridgeFeeConfigured` 对齐，独立 timelock op_hash）
#[event]
pub struct BridgeFeeConfigured {
    pub fee: u64,
}

/// gasless 路径服务费变更（与 EVM `GaslessFeeConfigured` 对齐，受 24h timelock 保护；
/// 设为 0 即熔断 gasless 路径）
#[event]
pub struct GaslessFeeConfigured {
    pub fee: u64,
}

/// 管理员从金库提取代币
#[event]
pub struct TokenWithdrawn {
    pub mint: Pubkey,
    pub to: Pubkey,
    pub amount: u64,
}

/// 运维者地址变更
#[event]
pub struct OperatorUpdated {
    pub old_operator: Pubkey,
    pub new_operator: Pubkey,
}

/// 运维者跳过某 nonce，使其永远无法 unlock（接收端使用）
///
/// **leaf 版本**：删除 source_chain_id 字段（leaf 只有一个 source = peer_chain_id）
#[event]
pub struct NonceSkipped {
    pub nonce: u64,
}

/// 退款执行完成（两步退款第 2 步），退还锁定资金至原始 staker（发送端使用）
#[event]
pub struct Refunded {
    pub nonce: u64,
    /// 退款目标地址（原始 staker）
    pub to: Pubkey,
    pub amount: u64,
}

/// Operator 发起退款（两步退款第 1 步），开始 REFUND_DELAY 倒计时
#[event]
pub struct RefundInitiated {
    pub nonce: u64,
    pub to: Pubkey,
    pub amount: u64,
}

/// Admin 取消已发起的退款，阻止其执行
#[event]
pub struct RefundCancelled {
    pub nonce: u64,
}

/// Guardian 触发紧急冻结
#[event]
pub struct EmergencyFreezeActivated {
    pub triggered_by: Pubkey,
}

/// Recovery 执行恢复，更换 admin 并解冻
#[event]
pub struct RecoveryExecuted {
    pub old_admin: Pubkey,
    pub new_admin: Pubkey,
}

/// Recovery 地址变更
#[event]
pub struct RecoveryUpdated {
    pub old_recovery: Pubkey,
    pub new_recovery: Pubkey,
}

/// Timelock 被激活，此后关键管理操作需经过延迟期
#[event]
pub struct TimelockActivated {}

/// 操作已调度，等待 TIMELOCK_DELAY 后方可执行
#[event]
pub struct OperationScheduled {
    /// 操作负载的 SHA-256 哈希，作为操作的唯一标识
    pub op_hash: [u8; 32],
    /// 最早可执行时间戳
    pub eta: u64,
    /// 原始操作负载，用于链下验证和日志
    pub data: Vec<u8>,
}

/// 已调度操作通过延迟期验证并成功执行
#[event]
pub struct OperationExecuted {
    pub op_hash: [u8; 32],
}

/// 已调度操作被管理员取消
#[event]
pub struct OperationCancelled {
    pub op_hash: [u8; 32],
}
