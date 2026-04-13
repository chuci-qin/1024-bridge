use anchor_lang::prelude::*;

/// 系统允许的最大中继器数量。
/// 限制 gas/CU 消耗和治理过于分散；同时约束 CrossChainRequest 账户大小。
pub const MAX_RELAYERS: usize = 18;

/// 手续费上限（1_000_000_000 = 1000 USDC，6 位小数精度）。
/// 防止管理员误操作设置天价手续费。
pub const MAX_FEE: u64 = 1_000_000_000;

/// 管理操作的强制等待时间（24 小时）。
/// 激活后所有关键操作需经过此延迟才能执行，给社区和用户留出反应窗口。
pub const TIMELOCK_DELAY: u64 = 24 * 3600;

/// 操作的执行窗口期（48 小时）。
/// 超过 eta + TIMELOCK_GRACE_PERIOD 后操作过期需重新调度，
/// 防止被遗忘的操作在遥远未来被意外执行。
pub const TIMELOCK_GRACE_PERIOD: u64 = 48 * 3600;

/// 退款操作的强制等待时间（6 小时）。
/// operator 发起退款后需等待此延迟才能执行，防止密钥泄露后立即双花。
/// 延迟期间 admin 可取消退款。
pub const REFUND_DELAY: u64 = 6 * 3600;

/// 统一桥状态 PDA（多 Peer 版本）。
///
/// 同一个 PDA 同时承担"发送方"和"接收方"双重角色：
/// - 发送方：管理 stake（锁定）、退款
/// - 接收方：管理 confirm_event（解锁）、速率限制
///
/// 多 Peer 版本将 peer 相关配置（合约地址、手续费、per-chain 速率限制）
/// 移至独立的 PeerConfig PDA，本结构体仅保留全局配置和安全机制。
///
/// 安全机制包括：紧急冻结与恢复、全局滑动窗口速率限制、最低储备金检查、白名单中继器投票确认。
///
/// 角色体系（四角色分离）：
/// - admin（多签）：全权管理，所有配置操作
/// - guardian（EOA）：紧急冻结，冻结后只有 recovery 可恢复
/// - operator（EOA）：日常运维（skip_nonce / initiate_refund），泄露风险有界
/// - recovery（冷钱包）：仅在冻结后可更换 admin 并解冻
///
/// Seeds: `[b"bridge_state"]`
#[account]
pub struct BridgeState {
    // ─── 角色 ───────────────────────────────────────────────────────────
    /// 管理员地址（多签钱包），拥有所有配置权限
    pub admin: Pubkey,
    /// 守护者地址（EOA），仅有紧急冻结权限
    /// 设计意图：admin 使用多签保障安全性，guardian 使用 EOA 保障响应速度
    pub guardian: Pubkey,
    /// 运维者地址（EOA），负责 skip_nonce 和 initiate_refund
    /// 泄露风险有界：只能发起退款（需等 6h 方可执行），不能动金库，admin 可随时更换/取消退款
    pub operator: Pubkey,
    /// 恢复地址（冷钱包/硬件多签），仅在冻结后可更换 admin 并解冻
    /// 泄露风险有界：单独持有无法操作（需 guardian 先冻结），不能直接转移资金
    pub recovery: Pubkey,
    /// 两步管理员转移中的待接受地址（Pubkey::default() 表示无待定转移）
    pub pending_admin: Pubkey,

    // ─── 配置 ───────────────────────────────────────────────────────────
    /// 金库 PDA 地址，用作代币账户的权限（authority）
    pub vault: Pubkey,
    /// 金库 PDA 的 bump seed，存储后避免每次 CPI 调用时重新 find_program_address
    pub vault_bump: u8,
    /// USDC SPL 代币的铸币地址
    pub usdc_mint: Pubkey,
    /// 本链的链 ID
    pub local_chain_id: u64,

    // ─── 全局速率限制（滑动窗口算法） ───────────────────────────────────
    // 全局速率限制作为所有链路出金的总上限，与 PeerConfig 中的 per-chain 限制形成双层保护。

    /// 每个时间窗口内允许的最大解锁总额（0 表示不限制）
    pub max_unlock_per_window: u64,
    /// 时间窗口的持续时长（秒），与 max_unlock_per_window 配合使用
    pub window_duration: u64,
    /// 当前窗口的起始时间戳（unix timestamp）
    pub current_window_start: u64,
    /// 当前窗口内已使用的解锁额度
    pub current_window_usage: u64,
    /// 上一个窗口的使用量，用于滑动窗口的加权计算
    pub previous_window_usage: u64,
    /// 单笔解锁的最大金额限制（0 表示不限制）
    pub max_single_unlock: u64,
    /// 金库需维持的最低储备金，unlock/execute_refund 后余额不得低于此值
    pub minimum_reserve: u64,

    // ─── 标志位 ─────────────────────────────────────────────────────────
    /// 桥是否已暂停。true 时所有 stake/unlock 操作被拒绝
    pub is_paused: bool,
    /// 时间锁是否已激活。初始部署阶段为 false，admin 完成配置后调用 activate_timelock 启用
    /// 一经激活不可撤销
    pub timelock_active: bool,

    // ─── 中继器 ─────────────────────────────────────────────────────────
    /// 白名单中继器列表。使用 Vec 动态管理，上限 MAX_RELAYERS。
    /// 线性扫描检查身份（O(n)），因 n ≤ 18 性能可接受。
    pub relayers: Vec<Pubkey>,
}

impl BridgeState {
    /// 账户所需的总空间（字节），包含 Anchor 的 8 字节鉴别器
    pub const LEN: usize = 8 // 鉴别器
        + 32 * 5  // 角色（admin, guardian, operator, recovery, pending_admin）
        + 32 * 2  // vault + usdc_mint
        + 1       // vault_bump
        + 8       // local_chain_id
        + 8 * 7   // 全局速率限制（7 个 u64）
        + 1 * 2   // 标志位（is_paused + timelock_active）
        + (4 + MAX_RELAYERS * 32); // relayers vec（4 字节长度前缀 + 最多 18 个 Pubkey）

    /// 检查指定地址是否为白名单中继器
    pub fn is_relayer(&self, addr: &Pubkey) -> bool {
        self.relayers.iter().any(|r| r == addr)
    }
}

/// 每条 Peer 链路的独立配置 PDA。
///
/// 每个已注册的对端链对应一个 PeerConfig PDA，存储该链路的合约地址、手续费和 per-chain 速率限制。
/// PDA 存在即表示链路可用，通过 `unregister_peer` 关闭 PDA 即下线链路。
/// Anchor 的账户反序列化天然保证了 PDA 不存在时交易 revert，无需额外 is_active 标志位。
///
/// Seeds: `[b"peer_config", chain_id.to_le_bytes()]`
#[account]
pub struct PeerConfig {
    /// 对端链的链 ID
    pub chain_id: u64,
    /// 对端桥合约地址（EVM 为右对齐 20B 的 bytes32，SVM 为原生 32B 公钥）
    pub peer_contract: [u8; 32],
    /// 该链路的桥手续费（USDC 原始精度），在 stake 和 unlock 时双向扣除
    /// 扣除的手续费留在金库作为协议收入
    pub bridge_fee: u64,
    /// 单笔 stake 的最大金额限制（0 表示不限制）
    /// 应配置为对端链 max_single_unlock 的值，防止用户 stake 后在对端无法 unlock
    pub max_stake_amount: u64,

    // ─── per-chain 速率限制（滑动窗口算法） ──────────────────────────────
    // 与 BridgeState 全局速率限制形成双层保护。
    // 即使某条链路被攻击，损失也被限制在该链路的窗口限额内。

    /// 该链路每个时间窗口内允许的最大解锁总额（0 表示不限制）
    pub max_unlock_per_window: u64,
    /// 该链路时间窗口的持续时长（秒）
    pub window_duration: u64,
    /// 该链路单笔解锁的最大金额限制（0 表示不限制）
    pub max_single_unlock: u64,
    /// 该链路当前窗口的起始时间戳
    pub current_window_start: u64,
    /// 该链路当前窗口内已使用的解锁额度
    pub current_window_usage: u64,
    /// 该链路上一个窗口的使用量
    pub previous_window_usage: u64,
}

impl PeerConfig {
    /// 账户所需的总空间（字节），包含 Anchor 的 8 字节鉴别器
    pub const LEN: usize = 8 // 鉴别器
        + 8       // chain_id
        + 32      // peer_contract
        + 8 * 8;  // bridge_fee + max_stake_amount + 6 个速率限制 u64
}

/// 每笔 stake 的链上记录，用于 refund 时验证退款金额和退款地址。
/// 金额和地址从链上记录读取，不可篡改。
///
/// Seeds: `[b"stake_record", nonce.to_le_bytes()]`
#[account]
pub struct StakeRecord {
    /// 原始 staker 地址，refund 只能退回给此地址
    pub owner: Pubkey,
    /// 用户实付全额（含手续费），退款时退还此金额（USDC 原始精度 6 位小数）
    /// 注意：StakeEvent.amount 为扣除 bridge_fee 后的净额，用于对端链 unlock
    pub amount: u64,
    /// 目标链 ID，用于审计和退款追踪
    pub target_chain_id: u64,
    /// 是否已退款，防止重复退款
    pub refunded: bool,
    /// 退款发起时间戳（unix timestamp），0 表示未发起
    /// 两步退款：operator 发起 → 等待 REFUND_DELAY → operator 或 staker 执行
    pub refund_initiated_at: u64,
}

impl StakeRecord {
    pub const LEN: usize = 8 + 32 + 8 + 8 + 1 + 8;
}

/// 哈希投票条目：记录有多少中继器为某个事件数据哈希投票。
///
/// 投票机制说明：每个中继器提交的 event_data 独立取 SHA-256 哈希。
/// 相比"第一个人说了算"，投票机制允许少数中继器提交错误数据而不影响正常流程——
/// 只要多数中继器提交相同的正确数据，就能达到阈值触发解锁。
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct HashVote {
    /// 事件数据的 SHA-256 哈希
    pub data_hash: [u8; 32],
    /// 该哈希获得的投票数
    pub count: u8,
}

/// 每个 nonce 的跨链请求 PDA，追踪确认收集和哈希投票以解锁代币。
///
/// 生命周期：
/// 1. 首个中继器调用 confirm_event 时通过 init_if_needed 创建，由该中继器支付租金
/// 2. 后续中继器继续投票，达到 2/3 阈值后触发解锁
/// 3. 解锁 / skip 完成时自动 compact：清空投票数据、缩小 PDA、退还多余租金给触发者
///
/// PDA 永不关闭（防止 init_if_needed 重建导致双花），compact 后保留标志位。
///
/// Seeds: `[b"cross_chain_request", source_chain_id.to_le_bytes(), nonce.to_le_bytes()]`
///
/// 多 Peer 版本在 seeds 中加入 source_chain_id，隔离不同源链的 nonce 空间，
/// 消除不同链用户生成相同随机 nonce 时的碰撞风险。
#[account]
pub struct CrossChainRequest {
    /// 该请求对应的跨链事件 nonce
    pub nonce: u64,
    /// 已确认的中继器列表，防止重复投票（完成后自动清空以节省租金）
    pub confirmed_relayers: Vec<Pubkey>,
    /// 哈希投票桶：不同的 event_data 哈希各自累计票数（完成后自动清空）
    pub hash_votes: Vec<HashVote>,
    /// 创建时冻结的解锁阈值 = ceil(2 * relayer_count / 3)
    /// 冻结在首次确认时，后续中继器数量变化不影响进行中的投票
    pub frozen_threshold: u8,
    /// 代币是否已解锁给接收者（仅解锁时为 true，skip 时保持 false）
    pub is_unlocked: bool,
    /// 此 nonce 是否已处理（unlock 或 skip 均设为 true），对应 EVM 的 processedNonces
    pub is_processed: bool,
}

impl CrossChainRequest {
    /// 创建时分配的完整空间（投票进行中需要存储所有中继器和哈希投票）
    pub const LEN: usize = 8 // 鉴别器
        + 8                           // nonce
        + (4 + MAX_RELAYERS * 32)     // confirmed_relayers（最多 18 个 Pubkey）
        + (4 + MAX_RELAYERS * (32+1)) // hash_votes（最多 18 个 HashVote：32B hash + 1B count）
        + 1                           // frozen_threshold
        + 1                           // is_unlocked
        + 1;                          // is_processed

    /// compact 后的最小空间（清空 confirmed_relayers 和 hash_votes 后）。
    /// 仅保留 nonce + is_unlocked + is_processed 防止重用，Vec 序列化为 4 字节长度前缀。
    pub const COMPACTED_LEN: usize = 8 // 鉴别器
        + 8                           // nonce
        + 4                           // confirmed_relayers（空 Vec 仅长度前缀）
        + 4                           // hash_votes（空 Vec 仅长度前缀）
        + 1                           // frozen_threshold
        + 1                           // is_unlocked
        + 1;                          // is_processed
}

/// 时间锁操作 PDA，由 schedule_operation 创建，由实际管理指令消费。
/// 消费时验证 eta + 宽限期，然后关闭账户（退还租金给 admin）。
///
/// Seeds: `[b"timelock", op_hash.as_ref()]`
#[account]
pub struct TimelockOperation {
    /// 最早可执行时间戳 = 调度时间 + TIMELOCK_DELAY
    pub eta: u64,
    /// 操作负载的 SHA-256 哈希，作为操作的唯一标识
    pub op_hash: [u8; 32],
}

impl TimelockOperation {
    pub const LEN: usize = 8 + 8 + 32;
}

/// 跨链事件数据，与 EVM 的 StakeEventData 结构完全对齐。
/// 所有字段使用固定宽度类型实现全定长序列化，保证跨链哈希一致性。
///
/// 在 confirm_event 中，中继器提交此数据，合约对其取 SHA-256 哈希后进行投票计数。
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq)]
pub struct StakeEventData {
    /// 源链桥合约地址
    pub source_contract: [u8; 32],
    /// 目标链桥合约地址
    pub target_contract: [u8; 32],
    /// 源链 ID
    pub source_chain_id: u64,
    /// 目标链 ID
    pub target_chain_id: u64,
    /// stake 发生时的区块/slot 高度
    pub block_height: u64,
    /// 锁定金额（USDC 原始精度，6 位小数）
    pub amount: u64,
    /// 发送者地址（32 字节）
    pub sender: [u8; 32],
    /// 接收者地址（EVM 右对齐 20B，SVM 原生 32B）
    pub receiver: [u8; 32],
    /// 客户端生成的随机唯一事件编号，防重放
    pub nonce: u64,
}

impl StakeEventData {
    /// Borsh 序列化后的固定大小：4 × 32B + 5 × 8B = 168 字节
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
