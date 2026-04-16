// SPDX-License-Identifier: MIT
pragma solidity 0.8.20;

import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";

/// @title Bridge1024 EVM 跨链桥合约
/// @notice 本合约是 Bridge1024 跨链桥的 EVM 端实现，支持 stake（锁定）和 unlock（解锁）两种核心操作。
/// 用户在源链 stake USDC 后，中继者（relayer）在目标链提交确认，达到 2/3 投票阈值后自动触发 unlock。
/// 合约同时承担"发送方"和"接收方"双重角色。
/// 安全机制包括：紧急冻结与恢复、防重入、滑动窗口速率限制、最低储备金检查、白名单中继者投票确认。
/// 角色体系：admin（多签，全权管理）、guardian（EOA，紧急冻结）、operator（EOA，skipNonce/退款发起 运维）、
/// recovery（冷钱包，仅冻结后可更换 admin 并解冻）。
contract Bridge1024 is Pausable, ReentrancyGuard {
    using SafeERC20 for IERC20;
    using SafeCast for uint256;

    // ─── 常量 ───────────────────────────────────────────────────────────
    /// @notice 系统允许的最大中继者数量，防止 gas 消耗过高和治理过于分散
    uint8 public constant MAX_RELAYERS = 18;
    /// @notice 本合约地址的 bytes32 右对齐表示，构造时缓存避免重复计算
    bytes32 private immutable SELF_BYTES32;
    // ─── 自定义错误 ─────────────────────────────────────────────────────
    // 使用自定义 error 替代 require+字符串，节省部署和调用 gas

    /// @notice 调用者不具备所需权限（非管理员或非白名单中继者）
    error Unauthorized();
    /// @notice 传入了零地址，关键地址参数不允许为空
    error ZeroAddress();
    /// @notice USDC 合约地址尚未配置，需要管理员先调用 configure
    error UsdcNotConfigured();

    /// @notice 试图添加已存在的中继者地址
    error RelayerAlreadyExists();
    /// @notice 中继者数量已达上限 MAX_RELAYERS
    error TooManyRelayers();
    /// @notice 指定的中继者地址不在白名单中
    error RelayerNotFound();
    /// @notice 该 nonce 对应的跨链事件已被处理过，防止重放
    error AlreadyProcessed();
    /// @notice 事件数据中的源合约地址与配置不匹配
    error InvalidSourceContract();
    /// @notice 事件数据中的目标合约地址与本合约不匹配
    error InvalidTargetContract();
    /// @notice 事件数据中的源链 ID 与配置不匹配
    error InvalidSourceChainId();
    /// @notice 事件数据中的目标链 ID 与本链不匹配
    error InvalidTargetChainId();
    /// @notice 解锁金额超出滑动窗口速率限制
    error RateLimitExceeded();
    /// @notice 单笔解锁金额超出最大单笔限额
    error SingleTransferExceeded();
    /// @notice 金库余额不足以支付解锁金额并维持最低储备金
    error InsufficientReserve();
    /// @notice 该中继者已对此 nonce 提交过确认，不允许重复确认
    error RelayerAlreadyConfirmed();
    /// @notice 金额为零，stake 和 unlock 均不允许零金额操作
    error ZeroAmount();
    /// @notice stake 金额超出最大单笔限额（应匹配对端链的 maxSingleUnlock）
    error StakeAmountExceeded();
    /// @notice 该 nonce 已被退款，不允许重复退款
    error AlreadyRefunded();
    /// @notice 操作未经 Timelock 调度，不能直接执行
    error TimelockNotScheduled();
    /// @notice Timelock 延迟时间未到，操作尚不可执行
    error TimelockNotReady();
    /// @notice 该操作已被调度，不允许重复调度
    error TimelockAlreadyScheduled();
    /// @notice Timelock 已经激活，不可重复激活
    error TimelockAlreadyActive();
    /// @notice Timelock 尚未激活，无需调度操作
    error TimelockNotActive();
    /// @notice 操作已超过执行窗口期（grace period），需重新调度
    error TimelockExpired();
    /// @notice 速率限制参数组合不合理
    error InvalidRateLimitParams();
    /// @notice configure 中链 ID 参数无效（零值或自环）
    error InvalidChainId();
    /// @notice receiver 的 bytes32 高 12 字节非零，不是合法的 EVM 地址格式
    error InvalidReceiver();
    /// @notice ETH 转账失败
    error ETHTransferFailed();
    /// @notice 构造函数中角色地址存在重叠，违反角色分离原则
    error RoleOverlap();
    /// @notice 该 nonce 已被使用，客户端应使用不同的随机 nonce
    error NonceAlreadyUsed();
    /// @notice 退款尚未发起（第一步），不能直接执行第二步
    error RefundNotInitiated();
    /// @notice 退款延迟时间未到，需等待 REFUND_DELAY 后方可执行
    error RefundNotReady();
    /// @notice 该 nonce 的退款已发起，不允许重复发起
    error RefundAlreadyInitiated();

    // ─── 数据结构 ─────────────────────────────────────────────────────

    /// @notice 跨链 stake 事件的完整数据，用于中继者确认和目标链 unlock
    /// 所有字段使用 bytes32/uint64 实现全定长序列化，跨链统一格式（兼容 SVM 端）
    struct StakeEventData {
        bytes32 sourceContract; // 源链桥合约地址
        bytes32 targetContract; // 目标链桥合约地址
        uint64 sourceChainId; // 源链 ID
        uint64 targetChainId; // 目标链 ID
        uint64 blockHeight; // stake 发生时的区块高度
        uint64 amount; // 锁定金额（USDC 原始精度，6 位小数）
        bytes32 sender; // 发送者地址（右对齐 bytes32）
        bytes32 receiver; // 接收者地址（EVM 右对齐 20B，SVM 原生 32B）
        uint64 nonce; // 客户端生成的随机唯一事件编号，防重放
    }

    /// @notice 按 nonce 聚合的确认收集状态，采用投票机制跟踪中继者对跨链事件的确认
    /// 每个 relayer 提交的 eventData 取 keccak256 哈希后投票，达到 frozenThreshold 时触发 unlock
    /// 相比"第一个人说了算"，投票机制允许少数 relayer 提交错误数据而不影响正常流程
    struct NonceConfirmation {
        mapping(address => bool) confirmedRelayers; // 记录哪些中继者已确认，防重复
        mapping(bytes32 => uint8) hashVotes; // eventData 哈希 → 投票数
        bool isProcessed; // 是否已处理（unlock 或 skipNonce），防止重复操作
        bool isUnlocked; // 是否已实际解锁转账（仅 unlock 时为 true，skip 时为 false）
        uint8 frozenThreshold; // 创建时冻结的解锁阈值（当时的 2/3 中继者数）
    }

    /// @notice 每笔 stake 的链上记录，用于 refund 时验证退款金额和退款地址
    /// 三个字段打包在同一个 storage slot（address 20B + uint64 8B + bool 1B = 29B）
    struct StakeRecord {
        address owner; // 原始 staker 地址，refund 只能退回给此地址
        uint64 amount; // 锁定金额（USDC 原始精度，6 位小数）
        bool refunded; // 是否已退款，防止重复退款
    }

    // ─── 状态变量：角色地址 ─────────────────────────────────────────────

    // ─── 以下两个变量打包在同一个 storage slot（20 + 8 = 28B） ───
    /// @notice 管理员地址（多签钱包），部署者自动成为初始 admin
    address public admin;
    /// @notice 本链的链 ID
    uint64 public localChainId;

    // ─── 以下两个变量打包在同一个 storage slot（20 + 8 = 28B） ───
    /// @notice USDC 代币合约地址
    address public usdcContract;
    /// @notice 对端链的链 ID
    uint64 public peerChainId;

    /// @notice 两步管理员转移中的待接受地址
    address public pendingAdmin;

    /// @notice 对端桥合约地址
    bytes32 public peerContract;

    /// @notice 中继者白名单数组（public getter 按 index 访问，长度通过 getRelayerCount() 查询）
    address[] public relayers;

    // ─── 以下两个变量打包在同一个 storage slot（20 + 1 = 21B） ───
    /// @notice 守护者地址（EOA），仅有紧急冻结权限，冻结后只有 recovery 可恢复
    /// 设计意图：admin 使用多签钱包保障安全性，guardian 使用 EOA 保障响应速度
    address public guardian;
    /// @notice 时间锁是否已激活，初始部署阶段为 false，admin 完成初始配置后调用 activateTimelock 启用
    bool public timelockActive;

    /// @notice 运维者地址（EOA），负责日常运维操作（skipNonce、initiateRefund）
    /// 泄露风险有界：只能发起退款（需等 6h 方可执行），不能动金库，admin 可随时更换/取消退款
    address public operator;

    /// @notice 恢复地址（冷钱包/硬件多签），仅在紧急冻结后可更换 admin 并解冻
    /// 泄露风险有界：单独持有无法操作（需 guardian 先冻结），不能直接转移资金
    address public recovery;

    // ─── 速率限制变量（滑动窗口算法） ─────────────────────────────────
    // 采用滑动窗口而非固定窗口，避免窗口边界处的突发解锁问题
    // 使用 uint64 足以表示所有 USDC 金额（最大 ~1840 万亿 USDC）和时间戳（~5840 亿年）
    // 以下 4 个配置变量打包在同一个 storage slot（4 × 8B = 32B）

    /// @notice 每个时间窗口内允许的最大解锁总额（为 0 表示不限制）
    uint64 public maxUnlockPerWindow;
    /// @notice 时间窗口的持续时长（秒），与 maxUnlockPerWindow 配合使用
    uint64 public windowDuration;
    /// @notice 单笔解锁的最大金额限制（为 0 表示不限制）
    uint64 public maxSingleUnlock;
    /// @notice 单笔 stake 的最大金额限制（为 0 表示不限制）
    /// 应配置为对端链 maxSingleUnlock 的值，防止用户 stake 后在对端无法 unlock
    uint64 public maxStakeAmount;

    // ─── 以下 4 个运行时变量打包在同一个 storage slot（4 × 8B = 32B） ───

    /// @notice 当前窗口的起始时间戳
    uint64 public currentWindowStart;
    /// @notice 当前窗口内已使用的解锁额度
    uint64 public currentWindowUsage;
    /// @notice 上一个窗口的使用量，用于滑动窗口的加权计算
    uint64 public previousWindowUsage;
    /// @notice 金库需维持的最低储备金，unlock 后余额不得低于此值
    uint64 public minimumReserve;

    // ─── 时间锁常量 ─────────────────────────────────────────────────────

    /// @notice 管理操作的强制等待时间，激活后所有关键操作需经过此延迟才能执行
    uint64 public constant TIMELOCK_DELAY = 24 hours;
    /// @notice 操作的执行窗口期，超过 eta + TIMELOCK_GRACE_PERIOD 后操作过期需重新调度
    uint64 public constant TIMELOCK_GRACE_PERIOD = 48 hours;
    /// @notice 退款操作的强制等待时间，operator 发起退款后需等待此延迟才能执行
    /// 防止 operator 密钥泄露后立即执行退款造成双花
    uint64 public constant REFUND_DELAY = 6 hours;
    /// @notice 已调度操作的最早可执行时间戳，opHash => eta（0 表示未调度）
    mapping(bytes32 => uint64) public timelockEta;

    /// @notice 按 nonce 存储投票确认进度（哈希投票计数和已确认的中继者）
    /// isProcessed 字段同时承担原 processedNonces 的职责，节省每个 nonce 一个 storage slot
    mapping(uint64 => NonceConfirmation) public nonceConfirmations;
    /// @notice 每笔 stake 的链上记录（owner + amount + refunded 打包在 1 slot）
    mapping(uint64 => StakeRecord) public stakes;
    /// @notice 退款发起时间戳，nonce => 发起时间（0 表示未发起）
    /// 两步退款机制：operator 发起 → 等待 REFUND_DELAY → operator 或 staker 执行
    mapping(uint64 => uint64) public refundInitiatedAt;

    // ─── 事件 ───────────────────────────────────────────────────────────

    /// @notice 用户 stake（锁定）USDC 时触发，中继者监听此事件以发起目标链解锁
    event StakeEvent(
        bytes32 indexed sourceContract,
        bytes32 indexed targetContract,
        uint64 sourceChainId,
        uint64 targetChainId,
        uint64 blockHeight,
        uint64 amount,
        bytes32 sender,
        bytes32 receiver,
        uint64 nonce
    );
    /// @notice 新中继者加入白名单
    event RelayerAdded(address indexed relayer);
    /// @notice 中继者从白名单移除
    event RelayerRemoved(address indexed relayer);
    /// @notice 中继者为某 nonce 提交了确认，dataHash 用于监控投票一致性
    event EventConfirmed(
        address indexed relayer,
        uint64 indexed nonce,
        bytes32 dataHash
    );
    /// @notice 确认达到阈值后成功解锁代币
    event TokensUnlocked(
        uint64 indexed nonce,
        address receiver,
        uint64 amount,
        bytes32 sender
    );
    /// @notice 守护者地址变更
    event GuardianUpdated(
        address indexed oldGuardian,
        address indexed newGuardian
    );
    /// @notice 管理员发起两步转移，提议新管理员
    event AdminTransferProposed(
        address indexed currentAdmin,
        address indexed pendingAdmin
    );
    /// @notice 新管理员接受转移，管理权正式交接
    event AdminTransferAccepted(
        address indexed oldAdmin,
        address indexed newAdmin
    );
    /// @notice 桥核心参数变更（USDC 地址、对端合约、链 ID）
    event BridgeConfigured(
        address indexed usdcContract,
        bytes32 peerContract,
        uint64 localChainId,
        uint64 peerChainId
    );
    /// @notice 速率限制参数变更
    event RateLimitsConfigured(
        uint64 maxUnlockPerWindow,
        uint64 windowDuration,
        uint64 maxSingleUnlock,
        uint64 maxStakeAmount,
        uint64 minimumReserve
    );
    /// @notice ETH 提取（处理被 selfdestruct 等强制发送的 ETH）
    event ETHWithdrawn(address indexed to, uint256 amount);
    /// @notice 管理员提取代币
    event TokenWithdrawn(
        address indexed token,
        address indexed to,
        uint256 amount
    );
    /// @notice 运维者地址变更
    event OperatorUpdated(
        address indexed oldOperator,
        address indexed newOperator
    );
    /// @notice 运维者跳过某 nonce，使其永远无法 unlock（接收端使用）
    event NonceSkipped(uint64 indexed nonce);
    /// @notice 退款执行完成（两步退款第 2 步），退款至原始 staker 地址
    event Refunded(uint64 indexed nonce, address indexed to, uint256 amount);
    /// @notice Operator 发起退款（两步退款第 1 步），开始 REFUND_DELAY 倒计时
    event RefundInitiated(
        uint64 indexed nonce,
        address indexed owner,
        uint64 amount
    );
    /// @notice Admin 取消已发起的退款，阻止其执行
    event RefundCancelled(uint64 indexed nonce);
    /// @notice Guardian 触发紧急冻结
    event EmergencyFreezeActivated(address indexed triggeredBy);
    /// @notice Recovery 执行恢复，更换 admin 并解冻
    event RecoveryExecuted(address indexed oldAdmin, address indexed newAdmin);
    /// @notice Recovery 地址变更
    event RecoveryUpdated(
        address indexed oldRecovery,
        address indexed newRecovery
    );
    /// @notice Timelock 被激活，此后关键管理操作需经过延迟期
    event TimelockActivated();
    /// @notice 操作已调度，等待延迟期后方可执行
    event OperationScheduled(bytes32 indexed opHash, uint64 eta, bytes data);
    /// @notice 已调度操作通过延迟期验证并成功执行
    event OperationExecuted(bytes32 indexed opHash);
    /// @notice 已调度操作被管理员取消
    event OperationCancelled(bytes32 indexed opHash);

    // ─── 修饰器 ─────────────────────────────────────────────────────

    /// @notice 限制只有管理员可调用
    modifier onlyAdmin() {
        if (msg.sender != admin) revert Unauthorized();
        _;
    }

    /// @notice 限制只有运维者可调用
    modifier onlyOperator() {
        if (msg.sender != operator) revert Unauthorized();
        _;
    }

    /// @notice 限制只有白名单中的中继者可调用，用于事件确认等关键操作
    modifier onlyWhitelistedRelayer() {
        if (!isRelayer(msg.sender)) revert Unauthorized();
        _;
    }

    // ─── 构造函数 ───────────────────────────────────────────────────────

    /// @notice 初始化桥合约，部署者自动成为初始 admin，后续可通过 proposeAdmin 转移给多签
    /// @param _guardian 守护者地址（EOA），紧急冻结权限
    /// @param _operator 运维者地址（EOA），skipNonce/退款发起 运维
    /// @param _recovery 恢复地址（冷钱包），紧急冻结后用于更换 admin
    /// 金库始终为合约自身地址 address(this)
    constructor(
        address _guardian,
        address _operator,
        address _recovery
    ) {
        if (_guardian == address(0)) revert ZeroAddress();
        if (_operator == address(0)) revert ZeroAddress();
        if (_recovery == address(0)) revert ZeroAddress();
        if (
            msg.sender == _guardian ||
            msg.sender == _operator ||
            msg.sender == _recovery ||
            _guardian == _operator ||
            _guardian == _recovery ||
            _operator == _recovery
        ) revert RoleOverlap();
        admin = msg.sender;
        guardian = _guardian;
        operator = _operator;
        recovery = _recovery;
        SELF_BYTES32 = _addressToBytes32(address(this));
    }

    /// @notice 显式拒绝直接 ETH 转账，明确合约不接受 ETH 的意图
    receive() external payable {
        revert();
    }

    // ─── 时间锁函数 ───────────────────────────────────────────────────

    /// @notice 激活时间锁，此后 configure、configureRateLimits、withdrawToken、
    /// addRelayer、removeRelayer、rotateRelayer、proposeAdmin、setGuardian、setOperator、
    /// setRecovery 均需先调度再执行。
    /// 初始部署阶段不激活，允许管理员快速完成首次配置；一经激活不可撤销。
    function activateTimelock() external onlyAdmin whenNotPaused {
        if (timelockActive) revert TimelockAlreadyActive();
        timelockActive = true;
        emit TimelockActivated();
    }

    /// @notice 调度一个需要时间锁的操作，TIMELOCK_DELAY 后方可执行
    /// @param data ABI 编码的操作标识和参数，
    ///   例如 abi.encode("configure", usdcAddr, peerContract, localChainId, peerChainId)
    function scheduleOperation(
        bytes calldata data
    ) external onlyAdmin whenNotPaused {
        if (!timelockActive) revert TimelockNotActive();
        bytes32 opHash = keccak256(data);
        if (timelockEta[opHash] != 0) revert TimelockAlreadyScheduled();
        uint64 eta = block.timestamp.toUint64() + TIMELOCK_DELAY;
        timelockEta[opHash] = eta;
        emit OperationScheduled(opHash, eta, data);
    }

    /// @notice 取消一个已调度但尚未执行的操作
    /// @param opHash 待取消操作的哈希
    function cancelOperation(bytes32 opHash) external onlyAdmin {
        if (timelockEta[opHash] == 0) revert TimelockNotScheduled();
        delete timelockEta[opHash];
        emit OperationCancelled(opHash);
    }

    // ─── 管理员函数 ───────────────────────────────────────────────────

    /// @notice 一次性配置桥的核心参数：USDC 地址、对端合约、链 ID
    /// 这些参数在部署后通常只设置一次，合并为单个函数以减少交易次数并保证原子性
    /// ⚠️ 警告：修改 peerContract 或链 ID 会导致所有进行中的 NonceConfirmation 因校验不匹配而永久卡住，
    /// 受影响的 nonce 需通过 skipNonce + initiateRefund/executeRefund 流程处理退款。
    /// @param _usdcContract USDC 代币合约地址
    /// @param _peerContract 对端桥合约地址
    /// @param _localChainId 本链的链 ID
    /// @param _peerChainId 对端链的链 ID
    function configure(
        address _usdcContract,
        bytes32 _peerContract,
        uint64 _localChainId,
        uint64 _peerChainId
    ) external onlyAdmin whenNotPaused {
        if (_usdcContract == address(0)) revert ZeroAddress();
        if (_peerContract == bytes32(0)) revert ZeroAddress();
        if (_localChainId == 0 || _peerChainId == 0) revert InvalidChainId();
        if (_localChainId == _peerChainId) revert InvalidChainId();
        _consumeTimelock(
            keccak256(
                abi.encode(
                    "configure",
                    _usdcContract,
                    _peerContract,
                    _localChainId,
                    _peerChainId
                )
            )
        );
        usdcContract = _usdcContract;
        peerContract = _peerContract;
        localChainId = _localChainId;
        peerChainId = _peerChainId;
        emit BridgeConfigured(
            _usdcContract,
            _peerContract,
            _localChainId,
            _peerChainId
        );
    }

    /// @notice 一次性配置所有速率限制参数，避免多次交易带来的中间状态风险
    /// @param _maxPerWindow 每个时间窗口的最大解锁总额
    /// @param _windowDuration 时间窗口持续时长（秒）
    /// @param _maxSingle 单笔最大解锁金额
    /// @param _maxStake 单笔最大 stake 金额（应匹配对端链的 _maxSingle，防止 stake 后无法 unlock）
    /// @param _minReserve 金库最低储备金要求
    function configureRateLimits(
        uint64 _maxPerWindow,
        uint64 _windowDuration,
        uint64 _maxSingle,
        uint64 _maxStake,
        uint64 _minReserve
    ) external onlyAdmin whenNotPaused {
        if ((_maxPerWindow == 0) != (_windowDuration == 0))
            revert InvalidRateLimitParams();
        if (_maxPerWindow != 0 && _maxSingle != 0 && _maxSingle > _maxPerWindow)
            revert InvalidRateLimitParams();
        if (_maxPerWindow != 0 && _windowDuration != 0 && _windowDuration < 60)
            revert InvalidRateLimitParams();
        _consumeTimelock(
            keccak256(
                abi.encode(
                    "configureRateLimits",
                    _maxPerWindow,
                    _windowDuration,
                    _maxSingle,
                    _maxStake,
                    _minReserve
                )
            )
        );
        maxUnlockPerWindow = _maxPerWindow;
        windowDuration = _windowDuration;
        maxSingleUnlock = _maxSingle;
        maxStakeAmount = _maxStake;
        minimumReserve = _minReserve;
        currentWindowStart = block.timestamp.toUint64();
        currentWindowUsage = 0;
        previousWindowUsage = 0;
        emit RateLimitsConfigured(
            _maxPerWindow,
            _windowDuration,
            _maxSingle,
            _maxStake,
            _minReserve
        );
    }

    /// @notice 添加新的中继者到白名单
    /// 遍历检查防止重复添加，总数不得超过 MAX_RELAYERS
    /// @param relayerAddress 要添加的中继者地址
    function addRelayer(
        address relayerAddress
    ) external onlyAdmin whenNotPaused {
        if (relayerAddress == address(0)) revert ZeroAddress();
        _consumeTimelock(keccak256(abi.encode("addRelayer", relayerAddress)));
        if (relayers.length >= MAX_RELAYERS) revert TooManyRelayers();
        for (uint256 i = 0; i < relayers.length; i++) {
            if (relayers[i] == relayerAddress) revert RelayerAlreadyExists();
        }
        relayers.push(relayerAddress);
        emit RelayerAdded(relayerAddress);
    }

    /// @notice 从白名单移除中继者
    /// 使用"交换到末尾再 pop"的模式以节省 gas（不保证顺序）
    /// @param relayerAddress 要移除的中继者地址
    function removeRelayer(
        address relayerAddress
    ) external onlyAdmin whenNotPaused {
        _consumeTimelock(
            keccak256(abi.encode("removeRelayer", relayerAddress))
        );
        uint256 idx = type(uint256).max;
        for (uint256 i = 0; i < relayers.length; i++) {
            if (relayers[i] == relayerAddress) {
                idx = i;
                break;
            }
        }
        if (idx == type(uint256).max) revert RelayerNotFound();
        relayers[idx] = relayers[relayers.length - 1];
        relayers.pop();
        emit RelayerRemoved(relayerAddress);
    }

    /// @notice 原子化地替换一个中继者：移除旧的、添加新的，无需两步操作
    /// 遍历时同时检查旧地址是否存在和新地址是否冲突
    /// @param oldRelayer 要移除的中继者地址
    /// @param newRelayer 要添加的中继者地址
    function rotateRelayer(
        address oldRelayer,
        address newRelayer
    ) external onlyAdmin whenNotPaused {
        if (newRelayer == address(0)) revert ZeroAddress();
        _consumeTimelock(
            keccak256(abi.encode("rotateRelayer", oldRelayer, newRelayer))
        );
        uint256 idx = type(uint256).max;
        for (uint256 i = 0; i < relayers.length; i++) {
            if (relayers[i] == oldRelayer) idx = i;
            if (relayers[i] == newRelayer) revert RelayerAlreadyExists();
        }
        if (idx == type(uint256).max) revert RelayerNotFound();
        relayers[idx] = newRelayer;
        emit RelayerRemoved(oldRelayer);
        emit RelayerAdded(newRelayer);
    }

    /// @notice 发起两步管理员转移（第一步：提议），新管理员必须主动调用 acceptAdmin 接受
    /// 如需取消提议，使用 cancelOperation 取消对应的 timelock 调度
    function proposeAdmin(address newAdmin) external onlyAdmin whenNotPaused {
        if (newAdmin == address(0)) revert ZeroAddress();
        _consumeTimelock(keccak256(abi.encode("proposeAdmin", newAdmin)));
        pendingAdmin = newAdmin;
        emit AdminTransferProposed(admin, newAdmin);
    }

    /// @notice 完成两步管理员转移（第二步：接受）
    /// 只有 pendingAdmin 本人可以调用，确保新管理员确实控制该地址
    function acceptAdmin() external whenNotPaused {
        if (msg.sender != pendingAdmin) revert Unauthorized();
        if (
            msg.sender == guardian ||
            msg.sender == operator ||
            msg.sender == recovery
        ) revert RoleOverlap();
        address oldAdmin = admin;
        admin = msg.sender;
        pendingAdmin = address(0);
        emit AdminTransferAccepted(oldAdmin, msg.sender);
    }

    /// @notice 设置守护者地址
    /// @param _guardian 新的守护者地址（不允许零地址）
    function setGuardian(address _guardian) external onlyAdmin whenNotPaused {
        if (_guardian == address(0)) revert ZeroAddress();
        _consumeTimelock(keccak256(abi.encode("setGuardian", _guardian)));
        if (
            _guardian == admin ||
            _guardian == operator ||
            _guardian == recovery
        ) revert RoleOverlap();
        address old = guardian;
        guardian = _guardian;
        emit GuardianUpdated(old, _guardian);
    }

    /// @notice 设置运维者地址
    /// @param _operator 新的运维者地址（不允许零地址）
    function setOperator(address _operator) external onlyAdmin whenNotPaused {
        if (_operator == address(0)) revert ZeroAddress();
        _consumeTimelock(keccak256(abi.encode("setOperator", _operator)));
        if (
            _operator == admin ||
            _operator == guardian ||
            _operator == recovery
        ) revert RoleOverlap();
        address old = operator;
        operator = _operator;
        emit OperatorUpdated(old, _operator);
    }

    /// @notice Guardian 紧急冻结合约，暂停所有 stake 和 unlock 操作
    /// 冻结后 admin 无法解除，只有 recovery 地址可通过 executeRecovery 恢复
    /// Guardian 泄露的最坏情况是 DoS（需 recovery 解冻），不会丢失资金
    function emergencyFreeze() external whenNotPaused {
        if (msg.sender != guardian) revert Unauthorized();
        _pause();
        emit EmergencyFreezeActivated(msg.sender);
    }

    /// @notice Recovery 恢复合约：更换 admin、可选替换 guardian 并解除冻结
    /// 仅在紧急冻结状态下可调用，确保 recovery 地址不能在正常状态下越权
    /// 允许同时替换 guardian 以打破恶意 guardian 反复冻结的 DoS 循环：
    /// 若仅替换 admin，新 admin 需通过 setGuardian（24h timelock）才能换掉恶意 guardian，
    /// 在此期间恶意 guardian 可不断 freeze→recovery→freeze，造成持续服务中断
    /// @param newAdmin 新的管理员地址
    /// @param newGuardian 新的守护者地址（传 address(0) 表示保留当前 guardian）
    function executeRecovery(
        address newAdmin,
        address newGuardian
    ) external whenPaused {
        if (msg.sender != recovery) revert Unauthorized();
        if (newAdmin == address(0)) revert ZeroAddress();
        address finalGuardian = newGuardian != address(0)
            ? newGuardian
            : guardian;
        if (
            newAdmin == finalGuardian ||
            newAdmin == operator ||
            newAdmin == recovery
        ) revert RoleOverlap();
        if (
            newGuardian != address(0) &&
            (newGuardian == operator || newGuardian == recovery)
        ) revert RoleOverlap();
        address oldAdmin = admin;
        admin = newAdmin;
        pendingAdmin = address(0);
        if (newGuardian != address(0)) {
            address oldGuardian = guardian;
            guardian = newGuardian;
            emit GuardianUpdated(oldGuardian, newGuardian);
        }
        _unpause();
        emit RecoveryExecuted(oldAdmin, newAdmin);
    }

    /// @notice 设置 recovery 地址（受 timelock 保护）
    /// @param _recovery 新的恢复地址
    function setRecovery(address _recovery) external onlyAdmin whenNotPaused {
        if (_recovery == address(0)) revert ZeroAddress();
        _consumeTimelock(keccak256(abi.encode("setRecovery", _recovery)));
        if (
            _recovery == admin ||
            _recovery == guardian ||
            _recovery == operator
        ) revert RoleOverlap();
        address old = recovery;
        recovery = _recovery;
        emit RecoveryUpdated(old, _recovery);
    }

    /// @notice 提取合约中的任意 ERC20 代币（受 Timelock 保护）
    /// 用于处理误转入的代币或按需转移资金
    /// @param token 代币合约地址
    /// @param amount 提取数量
    /// @param to 接收地址
    function withdrawToken(
        address token,
        uint256 amount,
        address to
    ) external onlyAdmin whenNotPaused nonReentrant {
        if (to == address(0)) revert ZeroAddress();
        _consumeTimelock(
            keccak256(abi.encode("withdrawToken", token, amount, to))
        );
        IERC20(token).safeTransfer(to, amount);
        emit TokenWithdrawn(token, to, amount);
    }

    /// @notice 提取合约中被强制发送的 ETH（如 selfdestruct、coinbase 奖励）
    /// 合约通过 receive() 拒绝直接 ETH 转账，但无法阻止 selfdestruct 强制发送
    /// @param to ETH 接收地址
    function withdrawETH(
        address payable to
    ) external onlyAdmin whenNotPaused nonReentrant {
        if (to == address(0)) revert ZeroAddress();
        uint256 balance = address(this).balance;
        if (balance == 0) revert ZeroAmount();
        _consumeTimelock(keccak256(abi.encode("withdrawETH", to)));
        (bool success, ) = to.call{value: balance}("");
        if (!success) revert ETHTransferFailed();
        emit ETHWithdrawn(to, balance);
    }

    /// @notice 跳过某 nonce，将其永久标记为已处理（接收端使用）
    /// 用于 unlock 永久失败时，operator 封死该 nonce 的解锁可能，配合发送端退款流程退还用户资金
    /// ⚠️ 必须在对端链 initiateRefund 之前调用，否则存在双花风险
    /// @param nonce 要跳过的 nonce 编号
    function skipNonce(uint64 nonce) external onlyOperator whenNotPaused {
        if (nonceConfirmations[nonce].isProcessed) revert AlreadyProcessed();
        nonceConfirmations[nonce].isProcessed = true;
        emit NonceSkipped(nonce);
    }

    /// @notice 发起退款（两步退款的第 1 步，仅 operator 可调用）
    /// 记录发起时间戳，需等待 REFUND_DELAY 后才能执行第 2 步
    /// 时间窗口内 admin 可取消，用于防止 operator 密钥泄露后的双花攻击
    /// @param nonce 要退款的 stake nonce
    function initiateRefund(uint64 nonce) external onlyOperator whenNotPaused {
        StakeRecord storage record = stakes[nonce];
        if (record.owner == address(0)) revert ZeroAddress();
        if (record.refunded) revert AlreadyRefunded();
        if (refundInitiatedAt[nonce] != 0) revert RefundAlreadyInitiated();

        refundInitiatedAt[nonce] = block.timestamp.toUint64();
        emit RefundInitiated(nonce, record.owner, record.amount);
    }

    /// @notice 执行退款（两步退款的第 2 步，operator 或原始 staker 均可调用）
    /// 需等待 REFUND_DELAY 后方可执行，受速率限制和储备金约束
    /// ⚠️ 必须先在对端链 skipNonce 封死 unlock，再发起退款，否则存在双花风险
    /// @param nonce 要退款的 stake nonce
    function executeRefund(uint64 nonce) external whenNotPaused nonReentrant {
        StakeRecord storage record = stakes[nonce];
        if (record.owner == address(0)) revert ZeroAddress();
        if (msg.sender != operator && msg.sender != record.owner)
            revert Unauthorized();
        if (record.refunded) revert AlreadyRefunded();

        uint64 initiatedAt = refundInitiatedAt[nonce];
        if (initiatedAt == 0) revert RefundNotInitiated();
        if (block.timestamp < uint256(initiatedAt) + REFUND_DELAY)
            revert RefundNotReady();

        uint64 amount = record.amount;
        _checkTransferLimits(amount);

        record.refunded = true;
        delete refundInitiatedAt[nonce];
        IERC20(usdcContract).safeTransfer(record.owner, uint256(amount));
        emit Refunded(nonce, record.owner, amount);
    }

    /// @notice 取消已发起的退款（仅 admin 可调用，暂停时也可调用）
    /// 用于 operator 密钥泄露后阻止恶意退款执行
    /// @param nonce 要取消退款的 stake nonce
    function cancelRefund(uint64 nonce) external onlyAdmin {
        if (refundInitiatedAt[nonce] == 0) revert RefundNotInitiated();
        delete refundInitiatedAt[nonce];
        emit RefundCancelled(nonce);
    }

    // ─── 查询函数 ─────────────────────────────────────────────────────

    /// @notice 返回当前活跃的中继者数量
    /// @return 中继者数量
    function getRelayerCount() external view returns (uint256) {
        return relayers.length;
    }

    /// @notice 检查指定地址是否为白名单中继者
    /// @param addr 要检查的地址
    /// @return 是否为白名单中继者
    function isRelayer(address addr) public view returns (bool) {
        for (uint256 i = 0; i < relayers.length; i++) {
            if (relayers[i] == addr) return true;
        }
        return false;
    }

    /// @notice 返回桥的基础身份和配置信息
    function getBridgeInfo()
        external
        view
        returns (
            address _admin,
            address _guardian,
            address _operator,
            address _recovery,
            address _pendingAdmin,
            address _usdcContract,
            bytes32 _peerContract,
            uint64 _localChainId,
            uint64 _peerChainId,
            bool _paused,
            bool _timelockActive,
            uint256 _relayerCount
        )
    {
        return (
            admin, guardian, operator, recovery, pendingAdmin,
            usdcContract, peerContract, localChainId, peerChainId,
            paused(), timelockActive, relayers.length
        );
    }

    /// @notice 返回速率限制配置和当前滑动窗口运行时状态
    function getRateLimitStatus()
        external
        view
        returns (
            uint64 _maxUnlockPerWindow,
            uint64 _windowDuration,
            uint64 _maxSingleUnlock,
            uint64 _maxStakeAmount,
            uint64 _minimumReserve,
            uint64 _currentWindowStart,
            uint64 _currentWindowUsage,
            uint64 _previousWindowUsage
        )
    {
        return (
            maxUnlockPerWindow, windowDuration,
            maxSingleUnlock, maxStakeAmount, minimumReserve,
            currentWindowStart, currentWindowUsage, previousWindowUsage
        );
    }

    // ─── 公共函数 ───────────────────────────────────────────────────────

    /// @notice 用户将 USDC 锁定（stake）到桥合约中，发起跨链转移
    /// 使用"转账前后余额差"模式获取实际到账金额，兼容有手续费的代币
    /// nonce 由客户端生成随机值传入，合约校验唯一性（防止碰撞和重放）
    /// @param nonce 客户端生成的随机唯一编号，标识本次跨链事件
    /// @param amount 用户希望锁定的 USDC 数量
    /// @param receiver 目标链上接收者地址（EVM 右对齐 20B，SVM 原生 32B）
    function stake(
        uint64 nonce,
        uint256 amount,
        bytes32 receiver
    ) external whenNotPaused nonReentrant {
        if (amount == 0) revert ZeroAmount();
        if (receiver == bytes32(0)) revert ZeroAddress();
        if (usdcContract == address(0)) revert UsdcNotConfigured();
        if (stakes[nonce].owner != address(0)) revert NonceAlreadyUsed();

        uint64 stakeAmount;
        {
            IERC20 usdc = IERC20(usdcContract);
            uint256 balanceBefore = usdc.balanceOf(address(this));
            usdc.safeTransferFrom(msg.sender, address(this), amount);
            uint256 actualAmount = usdc.balanceOf(address(this)) -
                balanceBefore;
            if (actualAmount == 0) revert ZeroAmount();
            if (maxStakeAmount != 0 && actualAmount > maxStakeAmount)
                revert StakeAmountExceeded();
            stakeAmount = actualAmount.toUint64();
        }

        stakes[nonce] = StakeRecord(msg.sender, stakeAmount, false);

        emit StakeEvent(
            SELF_BYTES32,
            peerContract,
            localChainId,
            peerChainId,
            block.number.toUint64(),
            stakeAmount,
            _addressToBytes32(msg.sender),
            receiver,
            nonce
        );
    }

    /// @notice 中继者确认某笔跨链事件（投票机制）
    /// 每个 relayer 提交 eventData，合约对数据取哈希后投票计数
    /// 当同一哈希的投票数达到 2/3 阈值时，自动触发 USDC 解锁
    /// 少数 relayer 提交错误数据不影响正常流程，多数正确即可通过
    /// @param eventData 跨链事件数据
    function confirmEvent(
        StakeEventData calldata eventData
    ) external onlyWhitelistedRelayer whenNotPaused nonReentrant {
        if (eventData.amount == 0) revert ZeroAmount();
        if (uint256(eventData.receiver) >> 160 != 0) revert InvalidReceiver();
        if (address(uint160(uint256(eventData.receiver))) == address(0))
            revert ZeroAddress();
        if (eventData.targetContract != SELF_BYTES32)
            revert InvalidTargetContract();
        if (usdcContract == address(0)) revert UsdcNotConfigured();
        if (eventData.sourceContract != peerContract)
            revert InvalidSourceContract();
        if (eventData.sourceChainId != peerChainId)
            revert InvalidSourceChainId();
        if (eventData.targetChainId != localChainId)
            revert InvalidTargetChainId();

        NonceConfirmation storage confirmation = nonceConfirmations[
            eventData.nonce
        ];
        if (confirmation.isProcessed) revert AlreadyProcessed();
        if (confirmation.confirmedRelayers[msg.sender])
            revert RelayerAlreadyConfirmed();

        if (confirmation.frozenThreshold == 0)
            confirmation.frozenThreshold = uint8((relayers.length * 2 + 2) / 3);

        confirmation.confirmedRelayers[msg.sender] = true;

        bytes32 dataHash = keccak256(
            abi.encodePacked(
                eventData.sourceContract,
                eventData.targetContract,
                eventData.sourceChainId,
                eventData.targetChainId,
                eventData.blockHeight,
                eventData.amount,
                eventData.sender,
                eventData.receiver,
                eventData.nonce
            )
        );
        confirmation.hashVotes[dataHash]++;

        emit EventConfirmed(msg.sender, eventData.nonce, dataHash);

        if (confirmation.hashVotes[dataHash] >= confirmation.frozenThreshold) {
            uint64 unlockAmount = eventData.amount;

            _checkTransferLimits(unlockAmount);

            address receiver = address(uint160(uint256(eventData.receiver)));

            confirmation.isProcessed = true;
            confirmation.isUnlocked = true;

            IERC20(usdcContract).safeTransfer(
                receiver,
                uint256(unlockAmount)
            );

            emit TokensUnlocked(
                eventData.nonce,
                receiver,
                eventData.amount,
                eventData.sender
            );
        }
    }

    // ─── 内部函数 ─────────────────────────────────────────────────────

    /// @dev 统一的转出限额检查：速率限制 + 单笔限额 + 金库储备不变量
    /// @param amount 本次转出金额
    function _checkTransferLimits(uint64 amount) internal {
        _checkRateLimit(amount);
        if (maxSingleUnlock != 0 && amount > maxSingleUnlock)
            revert SingleTransferExceeded();
        _checkVaultInvariant(amount);
    }

    /// @dev 将 address 右对齐编码为 bytes32，用于跨链统一地址格式
    function _addressToBytes32(address addr) internal pure returns (bytes32) {
        return bytes32(uint256(uint160(addr)));
    }

    /// @dev 消费时间锁：若 timelock 未激活则直接放行（初始部署阶段）；
    /// 若已激活则校验操作是否已调度且延迟期已过，通过后删除调度记录
    function _consumeTimelock(bytes32 opHash) internal {
        if (!timelockActive) return;
        uint64 eta = timelockEta[opHash];
        if (eta == 0) revert TimelockNotScheduled();
        if (block.timestamp < eta) revert TimelockNotReady();
        if (block.timestamp > eta + TIMELOCK_GRACE_PERIOD)
            revert TimelockExpired();
        delete timelockEta[opHash];
        emit OperationExecuted(opHash);
    }

    /// @dev 滑动窗口速率限制检查
    /// 相比固定窗口，滑动窗口通过加权上一窗口的剩余时间占比来平滑流量，
    /// 避免攻击者在两个固定窗口的交界处集中发起大量解锁
    /// @param amount 本次请求的解锁/退款金额
    function _checkRateLimit(uint64 amount) internal {
        uint256 _maxPerWindow = maxUnlockPerWindow;
        uint256 _duration = windowDuration;
        if (_maxPerWindow == 0 || _duration == 0) return;

        uint256 currentTime = block.timestamp;
        uint256 _windowStart = currentWindowStart;

        if (currentTime >= _windowStart + _duration) {
            if (currentTime < _windowStart + 2 * _duration) {
                previousWindowUsage = currentWindowUsage;
            } else {
                previousWindowUsage = 0;
            }
            currentWindowUsage = 0;
            currentWindowStart = currentTime.toUint64();
            _windowStart = currentTime;
        }

        uint256 elapsed = currentTime - _windowStart;
        uint256 remainingWeight = _duration - elapsed;
        uint256 slidingUsage = ((uint256(previousWindowUsage) *
            remainingWeight) / _duration) + currentWindowUsage;

        if (slidingUsage + amount > _maxPerWindow) revert RateLimitExceeded();

        currentWindowUsage += amount;
    }

    /// @dev 金库储备不变量检查：确保解锁后金库余额不低于最低储备金要求
    /// 这是最后一道安全防线，防止金库被完全掏空
    /// @param unlockAmount 本次请求的解锁/退款金额
    function _checkVaultInvariant(uint64 unlockAmount) internal view {
        uint256 vaultBalance = IERC20(usdcContract).balanceOf(
            address(this)
        );
        if (vaultBalance < uint256(unlockAmount) + minimumReserve)
            revert InsufficientReserve();
    }
}
