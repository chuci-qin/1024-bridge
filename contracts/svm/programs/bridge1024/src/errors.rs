use anchor_lang::prelude::*;

/// Bridge1024 程序的错误码枚举。
/// 每个变体对应一种具体的失败场景，便于前端和运维精确定位问题。
#[error_code]
pub enum ErrorCode {
    /// 调用者不具备所需权限（非管理员、非白名单中继器、角色不匹配）
    #[msg("Unauthorized")]
    Unauthorized,
    /// 传入了零地址（Pubkey::default()），关键地址参数不允许为空
    #[msg("Zero address")]
    ZeroAddress,
    /// USDC 铸币地址尚未配置，需要管理员先调用 configure
    #[msg("USDC mint not configured")]
    UsdcNotConfigured,
    /// 试图添加已存在的中继器地址
    #[msg("Relayer already exists")]
    RelayerAlreadyExists,
    /// 中继器数量已达上限 MAX_RELAYERS（18）
    #[msg("Too many relayers")]
    TooManyRelayers,
    /// 指定的中继器地址不在白名单中
    #[msg("Relayer not found")]
    RelayerNotFound,
    /// 该 nonce 对应的跨链请求已被处理过（已解锁或已跳过），防止重放
    #[msg("Already processed")]
    AlreadyProcessed,
    /// 事件数据中的源合约地址与配置的 peer_contract 不匹配
    #[msg("Invalid source contract")]
    InvalidSourceContract,
    /// 事件数据中的目标合约地址与本程序 ID 不匹配
    #[msg("Invalid target contract")]
    InvalidTargetContract,
    /// 事件数据中的源链 ID 与配置的 peer_chain_id 不匹配
    #[msg("Invalid source chain ID")]
    InvalidSourceChainId,
    /// 事件数据中的目标链 ID 与配置的 local_chain_id 不匹配
    #[msg("Invalid target chain ID")]
    InvalidTargetChainId,
    /// 解锁/退款金额超出滑动窗口速率限制
    #[msg("Rate limit exceeded")]
    RateLimitExceeded,
    /// 单笔解锁/退款金额超出 max_single_unlock 限额
    #[msg("Single transfer limit exceeded")]
    SingleTransferExceeded,
    /// 金库余额不足以支付转出金额并维持最低储备金
    #[msg("Insufficient reserve")]
    InsufficientReserve,
    /// 该中继器已对此 nonce 提交过确认，不允许重复投票
    #[msg("Relayer already confirmed")]
    RelayerAlreadyConfirmed,
    /// 金额为零，stake 和 unlock 均不允许零金额操作
    #[msg("Zero amount")]
    ZeroAmount,
    /// stake 金额超出 max_stake_amount（应匹配对端链的 max_single_unlock）
    #[msg("Stake amount exceeded")]
    StakeAmountExceeded,
    /// 该 nonce 已被退款，不允许重复退款
    #[msg("Already refunded")]
    AlreadyRefunded,
    /// 操作未经 Timelock 调度（PDA 不存在或不匹配），不能直接执行
    #[msg("Timelock not scheduled")]
    TimelockNotScheduled,
    /// Timelock 延迟时间未到（当前时间 < eta），操作尚不可执行
    #[msg("Timelock not ready")]
    TimelockNotReady,
    /// 该操作哈希的 Timelock PDA 已存在，不允许重复调度
    #[msg("Timelock already scheduled")]
    TimelockAlreadyScheduled,
    /// Timelock 已经激活，不可重复激活
    #[msg("Timelock already active")]
    TimelockAlreadyActive,
    /// Timelock 尚未激活，无法调度操作
    #[msg("Timelock not active")]
    TimelockNotActive,
    /// 操作已超过执行窗口期（eta + TIMELOCK_GRACE_PERIOD），需重新调度
    #[msg("Timelock expired")]
    TimelockExpired,
    /// 速率限制参数组合不合理（如窗口时长与限额必须同时为零或同时非零）
    #[msg("Invalid rate limit params")]
    InvalidRateLimitParams,
    /// 链 ID 参数无效（零值或自环，即 local_chain_id == peer_chain_id）
    #[msg("Invalid chain ID")]
    InvalidChainId,
    /// 接收者代币账户的 owner 与事件数据中的 receiver 不匹配
    #[msg("Invalid receiver")]
    InvalidReceiver,
    /// 角色地址存在重叠，违反四角色分离原则（admin/guardian/operator/recovery 互不相同）
    #[msg("Role overlap")]
    RoleOverlap,
    /// 桥已暂停，不允许执行需要未暂停状态的操作
    #[msg("Bridge is paused")]
    Paused,
    /// 桥未暂停，不允许执行需要暂停状态的操作（如 execute_recovery）
    #[msg("Bridge is not paused")]
    NotPaused,
    /// 跨链事件数据中 nonce 不匹配（confirm_event 校验）
    #[msg("Nonce mismatch")]
    NonceMismatch,
    /// 接收者代币账户的 mint 与桥配置的 USDC mint 不匹配
    #[msg("Receiver token account mint mismatch")]
    ReceiverMintMismatch,
    /// 手续费超出 MAX_FEE 上限
    #[msg("Fee too high")]
    FeeTooHigh,
    /// 转账后金库余额异常（实际到账金额为负），理论上不应发生
    #[msg("Insufficient balance")]
    InsufficientBalance,
    /// 事件数据无效（op_hash 与 data 的 SHA-256 不匹配）
    #[msg("Invalid event data")]
    InvalidEventData,
    /// 跨链请求尚未完成（未解锁/未跳过），不能关闭其 PDA
    #[msg("Request not completed yet")]
    RequestNotCompleted,
    /// 手续费大于或等于转账金额，扣除后净额为零
    #[msg("Fee exceeds transfer amount")]
    FeeExceedsAmount,
    /// 投票计数溢出（理论上不应发生，但优于 panic）
    #[msg("Nonce overflow")]
    NonceOverflow,
    /// 退款尚未发起（第一步），不能直接执行第二步
    #[msg("Refund not initiated")]
    RefundNotInitiated,
    /// 退款延迟时间未到，需等待 REFUND_DELAY 后方可执行
    #[msg("Refund delay not elapsed")]
    RefundNotReady,
    /// 该 nonce 的退款已发起，不允许重复发起
    #[msg("Refund already initiated")]
    RefundAlreadyInitiated,
    /// chain_id 等于 local_chain_id，不允许自环注册
    #[msg("Cannot register local chain as peer")]
    InvalidLocalChainId,
    /// confirm_event 中 source_chain_id 参数与 event_data.source_chain_id 不一致
    #[msg("Source chain ID mismatch")]
    SourceChainIdMismatch,
}
