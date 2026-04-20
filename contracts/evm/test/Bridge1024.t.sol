// SPDX-License-Identifier: MIT
pragma solidity 0.8.20;

// EVM 跨链桥合约 Bridge1024 的 Foundry 测试套件
// 覆盖初始化、发送方质押、接收方确认验证、速率限制、管理员权限及安全性等场景

import "forge-std/Test.sol";
import "../src/Bridge1024.sol";
import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

// ============ Mock Tokens ============

// 模拟 USDC 代币（6 位小数），用于测试桥合约的标准质押与解锁流程
contract MockUSDC is ERC20 {
    constructor() ERC20("USD Coin", "USDC") {}
    function decimals() public pure override returns (uint8) { return 6; }
    function mint(address to, uint256 amount) external { _mint(to, amount); }
}

// ============ Test Contract ============

contract Bridge1024Test is Test {
    // --- 合约实例 ---
    Bridge1024 public bridge;
    MockUSDC public usdc;

    // --- 管理员账户 ---
    address public admin;

    // --- 普通用户，用于模拟质押和接收代币 ---
    address public user1;
    address public user2;

    // --- 守护者（EOA），仅有紧急冻结权限 ---
    address public guardian;

    // --- 运维者（EOA），负责 skipNonce 和 refund ---
    address public oper;

    // --- 恢复地址（冷钱包），紧急冻结后用于更换 admin ---
    address public recovery;

    // --- 中继者账户（共 3 个），用于确认和阈值达标测试 ---
    address public relayer1;
    address public relayer2;
    address public relayer3;

    // --- 对端合约地址和链 ID，模拟跨链配对 ---
    bytes32 public peerContract = bytes32(uint256(0xdeadbeefcafe));
    uint64 public sourceChainId = 1;
    uint64 public targetChainId = 2;

    // 重新声明合约事件，供 vm.expectEmit 在测试中匹配事件日志
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
    event RelayerAdded(address indexed relayer);
    event RelayerRemoved(address indexed relayer);
    event EventConfirmed(address indexed relayer, uint64 indexed nonce, bytes32 dataHash);
    event TokensUnlocked(uint64 indexed nonce, address receiver, uint64 amount, bytes32 sender);
    event AdminTransferProposed(address indexed currentAdmin, address indexed pendingAdmin);
    event AdminTransferAccepted(address indexed oldAdmin, address indexed newAdmin);
    event GuardianUpdated(address indexed oldGuardian, address indexed newGuardian);
    event BridgeConfigured(address indexed usdcContract, bytes32 peerContract, uint64 localChainId, uint64 peerChainId);
    event RateLimitsConfigured(uint64 maxUnlockPerWindow, uint64 windowDuration, uint64 maxSingleUnlock, uint64 maxStakeAmount, uint64 minimumReserve);
    event TokenWithdrawn(address indexed token, address indexed to, uint256 amount);
    event OperatorUpdated(address indexed oldOperator, address indexed newOperator);
    event NonceSkipped(uint64 indexed nonce);
    event Refunded(uint64 indexed nonce, address indexed sender, uint256 amount);
    event RefundInitiated(uint64 indexed nonce, address indexed owner, uint64 amount);
    event RefundCancelled(uint64 indexed nonce);
    event TimelockActivated();
    event OperationScheduled(bytes32 indexed opHash, uint64 eta, bytes data);
    event OperationExecuted(bytes32 indexed opHash);
    event OperationCancelled(bytes32 indexed opHash);
    event ETHWithdrawn(address indexed to, uint256 amount);
    event EmergencyFreezeActivated(address indexed triggeredBy);
    event RecoveryExecuted(address indexed oldAdmin, address indexed newAdmin);
    event RecoveryUpdated(address indexed oldRecovery, address indexed newRecovery);

    // ============ setUp ============

    // 初始化测试环境：
    //   1. 生成管理员、用户、中继者账户
    //   2. 部署桥合约，配置 USDC 代币地址、对端合约和链 ID
    //   3. 设置速率限制为最大值（不限流）
    //   4. 添加 3 个中继者（阈值 = ceil(3*2/3) = 2）
    //   5. 为用户和桥合约铸造代币并完成授权
    function setUp() public {
        admin = makeAddr("admin");
        guardian = makeAddr("guardian");
        oper = makeAddr("operator");
        recovery = makeAddr("recovery");
        user1 = makeAddr("user1");
        user2 = makeAddr("user2");
        relayer1 = makeAddr("relayer1");
        relayer2 = makeAddr("relayer2");
        relayer3 = makeAddr("relayer3");

        vm.startPrank(admin);
        bridge = new Bridge1024(guardian, oper, recovery);
        usdc = new MockUSDC();

        bridge.configure(address(usdc), peerContract, sourceChainId, targetChainId);
        bridge.configureRateLimits(type(uint64).max, 3600, type(uint64).max, 0, 0);

        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();

        usdc.mint(user1, 10_000e6);
        usdc.mint(user2, 10_000e6);
        usdc.mint(address(bridge), 100_000e6);

        vm.prank(user1);
        usdc.approve(address(bridge), type(uint256).max);
        vm.prank(user2);
        usdc.approve(address(bridge), type(uint256).max);
    }

    // ============ Helpers ============

    // 计算 2/3 多数阈值：threshold = ceil(count * 2 / 3)
    function _calculateThreshold(uint256 count) internal pure returns (uint8) {
        return uint8((count * 2 + 2) / 3);
    }

    // 构造标准的 StakeEventData 结构体，填入对端合约、链 ID 等配置信息
    function _makeEventData(
        uint64 amount,
        address receiver,
        uint64 nonce
    ) internal view returns (Bridge1024.StakeEventData memory) {
        return Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: targetChainId,
            targetChainId: sourceChainId,
            blockHeight: 100,
            amount: amount,
            sender: bytes32(uint256(uint160(user1))),
            receiver: bytes32(uint256(uint160(receiver))),
            nonce: nonce
        });
    }

    function _isProcessed(uint64 nonce) internal view returns (bool) {
        (bool isProcessed, , ) = bridge.nonceConfirmations(nonce);
        return isProcessed;
    }

    // 批量提交确认直到达到阈值，触发代币解锁
    function _confirmToThreshold(Bridge1024.StakeEventData memory data) internal {
        uint8 threshold = _calculateThreshold(bridge.getRelayerCount());
        address[3] memory addrs = [relayer1, relayer2, relayer3];
        for (uint8 i = 0; i < threshold; i++) {
            vm.prank(addrs[i]);
            bridge.confirmEvent(data);
        }
    }

    // ========================================================================
    //                         INITIALIZATION TESTS
    // ========================================================================

    // 验证桥合约部署后 admin 是否正确设置
    function testInitialize() public view {
        assertEq(bridge.admin(), admin);
    }

    // 验证 configure 一次性设置所有核心参数：USDC、对端合约、链 ID
    // 同时验证零地址被拒绝、非管理员调用被拒绝
    function testConfigure() public {
        MockUSDC newUsdc = new MockUSDC();
        bytes32 newPeer = bytes32(uint256(0xabcdef));

        vm.expectEmit(true, false, false, true, address(bridge));
        emit BridgeConfigured(address(newUsdc), newPeer, 10, 20);

        vm.prank(admin);
        bridge.configure(address(newUsdc), newPeer, 10, 20);

        assertEq(bridge.admin(), admin);
        assertEq(bridge.usdcContract(), address(newUsdc));
        assertEq(bridge.peerContract(), newPeer);
        assertEq(bridge.localChainId(), 10);
        assertEq(bridge.peerChainId(), 20);

        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.configure(address(0), newPeer, 10, 20);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configure(address(newUsdc), newPeer, 10, 20);
    }

    // ========================================================================
    //                            SENDER TESTS (stake)
    // ========================================================================

    // 验证正常质押流程：用户余额扣减、桥合约余额增加、nonce 递增、事件正确触发
    function testStake_Success() public {
        uint256 amount = 500e6;
        bytes32 receiver = bytes32(uint256(0xdeadbeef));

        vm.roll(42);
        uint256 balBefore = usdc.balanceOf(user1);

        vm.expectEmit(true, true, false, true, address(bridge));
        emit StakeEvent(
            bytes32(uint256(uint160(address(bridge)))),
            peerContract,
            sourceChainId,
            targetChainId,
            42,
            uint64(amount),
            bytes32(uint256(uint160(user1))),
            receiver,
            1
        );

        vm.prank(user1);
        bridge.stake(1, amount, receiver);

        assertEq(usdc.balanceOf(user1), balBefore - amount);
        assertEq(usdc.balanceOf(address(bridge)), 100_000e6 + amount);
    }

    // 验证余额不足时质押应回退
    function testStake_InsufficientBalance() public {
        vm.prank(user1);
        vm.expectRevert();
        bridge.stake(1, 20_000e6, bytes32(uint256(1)));
    }

    // 验证未授权 USDC 转账时质押应回退
    function testStake_NotApproved() public {
        address user3 = makeAddr("user3");
        usdc.mint(user3, 1000e6);

        vm.prank(user3);
        vm.expectRevert();
        bridge.stake(1, 100e6, bytes32(uint256(1)));
    }

    // 验证 USDC 地址未配置时质押应回退
    function testStake_UsdcNotConfigured() public {
        vm.prank(admin);
        Bridge1024 freshBridge = new Bridge1024(guardian, oper, recovery);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.UsdcNotConfigured.selector);
        freshBridge.stake(1, 100e6, bytes32(uint256(1)));
    }

    // 验证合约冻结状态下质押应回退
    function testStake_WhenFrozen() public {
        vm.prank(guardian);
        bridge.emergencyFreeze();

        vm.prank(user1);
        vm.expectRevert(abi.encodeWithSignature("EnforcedPause()"));
        bridge.stake(1, 100e6, bytes32(uint256(1)));
    }

    // 验证接收地址为 bytes32(0) 时应回退
    function testStake_ZeroReceiver() public {
        vm.prank(user1);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.stake(1, 100e6, bytes32(0));
    }

    // ========================================================================
    //                        RECEIVER TESTS (confirmEvent)
    // ========================================================================

    // ---- Relayer management ----

    // 验证添加中继者：成功添加、零地址拒绝、重复添加拒绝
    function testAddRelayer() public {
        address newRelayer = makeAddr("newRelayer");

        vm.expectEmit(true, false, false, false, address(bridge));
        emit RelayerAdded(newRelayer);

        vm.prank(admin);
        bridge.addRelayer(newRelayer);
        assertTrue(bridge.isRelayer(newRelayer));

        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.addRelayer(address(0));

        vm.prank(admin);
        vm.expectRevert(Bridge1024.RelayerAlreadyExists.selector);
        bridge.addRelayer(newRelayer);
    }

    // 验证中继者数量达到上限（18个）后，再添加应回退
    function testAddRelayer_MaxRelayers() public {
        vm.startPrank(admin);
        for (uint256 i = 0; i < 15; i++) {
            bridge.addRelayer(address(uint160(0xF000 + i)));
        }
        assertEq(bridge.getRelayerCount(), 18);

        vm.expectRevert(Bridge1024.TooManyRelayers.selector);
        bridge.addRelayer(address(uint160(0xFFFF)));
        vm.stopPrank();
    }

    // 验证移除中继者：管理员可移除，非管理员调用应被拒绝
    function testRemoveRelayer() public {
        vm.prank(admin);
        vm.expectEmit(true, false, false, false, address(bridge));
        emit RelayerRemoved(relayer3);
        bridge.removeRelayer(relayer3);

        assertFalse(bridge.isRelayer(relayer3));
        assertEq(bridge.getRelayerCount(), 2);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.removeRelayer(relayer1);
    }

    // 验证原子轮换中继者：旧的移除、新的添加，总数不变
    function testRotateRelayer() public {
        address newRelayer = makeAddr("rotated");

        vm.prank(admin);
        bridge.rotateRelayer(relayer2, newRelayer);

        assertFalse(bridge.isRelayer(relayer2));
        assertTrue(bridge.isRelayer(newRelayer));
        assertEq(bridge.getRelayerCount(), 3);
    }

    // ---- Event confirmation ----

    // 验证单个中继者确认后不会触发解锁（未达阈值），nonce 仍为未处理状态
    function testConfirmEvent_SingleRelayer() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        vm.expectEmit(true, true, false, false, address(bridge));
        emit EventConfirmed(relayer1, 1, bytes32(0));

        vm.prank(relayer1);
        bridge.confirmEvent(data);

        assertFalse(_isProcessed(1));
        assertEq(usdc.balanceOf(user1), 10_000e6);
    }

    // 验证第二个中继者确认达到阈值后触发代币解锁，用户余额增加，nonce 标记为已处理
    function testConfirmEvent_ReachThreshold() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        vm.prank(relayer1);
        bridge.confirmEvent(data);

        uint256 userBalBefore = usdc.balanceOf(user1);

        vm.prank(relayer2);
        vm.expectEmit(true, false, false, true, address(bridge));
        emit TokensUnlocked(1, user1, 100e6, bytes32(uint256(uint160(user1))));
        bridge.confirmEvent(data);

        assertTrue(_isProcessed(1));
        assertEq(usdc.balanceOf(user1), userBalBefore + 100e6);
    }

    // 验证 nonce 支持乱序处理：先处理 nonce=3 再处理 nonce=1，互不影响
    function testConfirmEvent_NonceOutOfOrder() public {
        Bridge1024.StakeEventData memory data3 = _makeEventData(50e6, user1, 3);
        _confirmToThreshold(data3);
        assertTrue(_isProcessed(3));

        Bridge1024.StakeEventData memory data1 = _makeEventData(60e6, user1, 1);
        _confirmToThreshold(data1);
        assertTrue(_isProcessed(1));

        assertFalse(_isProcessed(2));
    }

    // 验证已处理的 nonce 再次提交确认时应回退，防止重放攻击
    function testConfirmEvent_ReplayBlocked() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        _confirmToThreshold(data);
        assertTrue(_isProcessed(1));

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.AlreadyProcessed.selector);
        bridge.confirmEvent(data);
    }

    // 验证非白名单中继者提交确认应被拒绝
    function testConfirmEvent_NonWhitelistedRelayer() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        address outsider = makeAddr("outsider");

        vm.prank(outsider);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.confirmEvent(data);
    }

    // 验证事件数据中的源合约地址与配置不匹配时应回退
    function testConfirmEvent_WrongSourceContract() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        data.sourceContract = bytes32(uint256(0x1234));

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidSourceContract.selector);
        bridge.confirmEvent(data);
    }

    // 验证事件数据中的源链 ID 与配置不匹配时应回退
    function testConfirmEvent_WrongSourceChainId() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        data.sourceChainId = 999;

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidSourceChainId.selector);
        bridge.confirmEvent(data);
    }

    // 验证事件数据中的目标链 ID 与本链不匹配时应回退
    function testConfirmEvent_WrongTargetChainId() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        data.targetChainId = 999;

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidTargetChainId.selector);
        bridge.confirmEvent(data);
    }

    // 验证事件数据中的目标合约地址与本合约不匹配时应回退
    function testConfirmEvent_WrongTargetContract() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        data.targetContract = bytes32(uint256(0x5678));

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidTargetContract.selector);
        bridge.confirmEvent(data);
    }

    // 投票机制：少数 relayer 提交错误数据不会阻止多数正确数据达成阈值
    // 场景：relayer1 提交被篡改的数据，relayer2+relayer3 提交正确数据，仍能解锁
    function testConfirmEvent_VotingMinorityWrongData() public {
        Bridge1024.StakeEventData memory correctData = _makeEventData(100e6, user1, 1);

        Bridge1024.StakeEventData memory tamperedData = _makeEventData(100e6, user1, 1);
        tamperedData.sender = bytes32(uint256(uint160(user2)));

        uint256 balBefore = usdc.balanceOf(user1);

        vm.prank(relayer1);
        bridge.confirmEvent(tamperedData);

        vm.prank(relayer2);
        bridge.confirmEvent(correctData);

        vm.prank(relayer3);
        bridge.confirmEvent(correctData);

        assertEq(usdc.balanceOf(user1) - balBefore, 100e6, "correct majority should trigger unlock");
    }

    // 投票机制：如果所有 relayer 提交的数据各不相同，任何版本都达不到阈值，不会解锁
    function testConfirmEvent_VotingNoConsensus() public {
        Bridge1024.StakeEventData memory data1 = _makeEventData(100e6, user1, 1);
        Bridge1024.StakeEventData memory data2 = _makeEventData(200e6, user1, 1);
        Bridge1024.StakeEventData memory data3 = _makeEventData(300e6, user1, 1);

        uint256 balBefore = usdc.balanceOf(user1);

        vm.prank(relayer1);
        bridge.confirmEvent(data1);
        vm.prank(relayer2);
        bridge.confirmEvent(data2);
        vm.prank(relayer3);
        bridge.confirmEvent(data3);

        assertEq(usdc.balanceOf(user1), balBefore, "no consensus should not trigger unlock");
    }

    // 投票机制：篡改 amount 的少数 relayer 不影响正确多数
    function testConfirmEvent_VotingTamperedAmount() public {
        Bridge1024.StakeEventData memory correctData = _makeEventData(100e6, user1, 1);
        Bridge1024.StakeEventData memory tamperedData = _makeEventData(200e6, user1, 1);

        uint256 balBefore = usdc.balanceOf(user1);

        vm.prank(relayer1);
        bridge.confirmEvent(tamperedData);

        vm.prank(relayer2);
        bridge.confirmEvent(correctData);

        vm.prank(relayer3);
        bridge.confirmEvent(correctData);

        assertEq(usdc.balanceOf(user1) - balBefore, 100e6, "correct majority should trigger unlock with correct amount");
    }

    // 验证阈值冻结机制：首次确认提交后添加新中继者，阈值仍按提交时的快照计算
    function testConfirmEvent_FrozenThreshold() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        vm.prank(relayer1);
        bridge.confirmEvent(data);

        address relayer4 = makeAddr("relayer4");
        vm.prank(admin);
        bridge.addRelayer(relayer4);
        assertEq(bridge.getRelayerCount(), 4);

        uint256 userBalBefore = usdc.balanceOf(user1);

        vm.prank(relayer2);
        vm.expectEmit(true, false, false, true, address(bridge));
        emit TokensUnlocked(1, user1, 100e6, bytes32(uint256(uint160(user1))));
        bridge.confirmEvent(data);

        assertEq(usdc.balanceOf(user1), userBalBefore + 100e6);
    }

    // 验证合约冻结状态下提交确认应回退
    function testConfirmEvent_WhenFrozen() public {
        vm.prank(guardian);
        bridge.emergencyFreeze();

        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        vm.prank(relayer1);
        vm.expectRevert(abi.encodeWithSignature("EnforcedPause()"));
        bridge.confirmEvent(data);
    }

    // 验证同一中继者对同一 nonce 重复确认应被拒绝
    function testConfirmEvent_DuplicateRelayer() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        vm.prank(relayer1);
        bridge.confirmEvent(data);

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.RelayerAlreadyConfirmed.selector);
        bridge.confirmEvent(data);
    }

    // ========================================================================
    //                          RATE LIMITING TESTS
    // ========================================================================

    // 验证窗口内累计解锁量超过限额时第二笔应回退
    function testRateLimit_ExceedWindow() public {
        vm.prank(admin);
        bridge.configureRateLimits(500e6, 3600, 0, 0, 0);

        Bridge1024.StakeEventData memory data1 = _makeEventData(400e6, user1, 1);
        _confirmToThreshold(data1);

        Bridge1024.StakeEventData memory data2 = _makeEventData(200e6, user1, 2);
        vm.prank(relayer1);
        bridge.confirmEvent(data2);

        vm.prank(relayer2);
        vm.expectRevert(Bridge1024.RateLimitExceeded.selector);
        bridge.confirmEvent(data2);
    }

    // 验证滑动窗口机制：等待两个完整窗口后，前一窗口用量完全衰减，新交易可以通过
    function testRateLimit_SlidingWindow() public {
        vm.prank(admin);
        bridge.configureRateLimits(500e6, 3600, 0, 0, 0);

        Bridge1024.StakeEventData memory data1 = _makeEventData(400e6, user1, 1);
        _confirmToThreshold(data1);

        vm.warp(block.timestamp + 7201);

        Bridge1024.StakeEventData memory data2 = _makeEventData(400e6, user1, 2);
        _confirmToThreshold(data2);

        assertTrue(_isProcessed(1));
        assertTrue(_isProcessed(2));
    }

    // 验证单笔交易超过最大限额时应回退。
    // confirmEvent 在入口处早拒，不等到阈值达成，避免 relayer 浪费 gas
    function testRateLimit_MaxSingle() public {
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, 500e6, 0, 0);

        Bridge1024.StakeEventData memory data = _makeEventData(600e6, user1, 1);
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.SingleTransferExceeded.selector);
        bridge.confirmEvent(data);
    }

    // 验证解锁后桥合约余额低于最低储备金时应回退，保护流动性
    function testRateLimit_MinReserve() public {
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, type(uint64).max, 0, 99_500e6);

        Bridge1024.StakeEventData memory data = _makeEventData(1000e6, user1, 1);
        vm.prank(relayer1);
        bridge.confirmEvent(data);

        vm.prank(relayer2);
        vm.expectRevert(Bridge1024.InsufficientReserve.selector);
        bridge.confirmEvent(data);
    }

    // ========================================================================
    //                            ADMIN TESTS
    // ========================================================================

    // 验证管理员可以提议新管理员，非管理员调用应被拒绝
    function testProposeAdmin() public {
        vm.expectEmit(true, true, false, false, address(bridge));
        emit AdminTransferProposed(admin, user1);

        vm.prank(admin);
        bridge.proposeAdmin(user1);

        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.proposeAdmin(address(0));

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.proposeAdmin(user2);
    }

    // 验证被提议人接受管理员权限后，旧管理员失去操作权限
    function testAcceptAdmin() public {
        vm.prank(admin);
        bridge.proposeAdmin(user1);

        vm.expectEmit(true, true, false, false, address(bridge));
        emit AdminTransferAccepted(admin, user1);

        vm.prank(user1);
        bridge.acceptAdmin();

        assertEq(bridge.admin(), user1);

        vm.prank(admin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configure(address(usdc), peerContract, sourceChainId, targetChainId);
    }

    // 验证未被提议的地址调用 acceptAdmin 应被拒绝
    function testAcceptAdmin_NotPending() public {
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.acceptAdmin();
    }

    // 验证紧急冻结完整流程：guardian 冻结 → recovery 恢复并更换 admin
    function testEmergencyFreeze_FullFlow() public {
        // 非 guardian 不能冻结
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.emergencyFreeze();

        // admin 也不能冻结（只有 guardian 可以）
        vm.prank(admin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.emergencyFreeze();

        // guardian 冻结合约
        vm.expectEmit(true, false, false, false, address(bridge));
        emit EmergencyFreezeActivated(guardian);
        vm.prank(guardian);
        bridge.emergencyFreeze();

        assertTrue(bridge.paused());

        // 冻结后 stake 被阻止
        vm.prank(user1);
        vm.expectRevert(abi.encodeWithSignature("EnforcedPause()"));
        bridge.stake(1, 100e6, bytes32(uint256(1)));

        // 非 recovery 不能恢复
        vm.prank(admin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.executeRecovery(user1, address(0));

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.executeRecovery(user1, address(0));

        // recovery 恢复并指定新 admin
        address newAdmin = makeAddr("newAdmin");
        vm.expectEmit(true, true, false, false, address(bridge));
        emit RecoveryExecuted(admin, newAdmin);
        vm.prank(recovery);
        bridge.executeRecovery(newAdmin, address(0));

        // 验证恢复后状态
        assertFalse(bridge.paused());
        assertEq(bridge.admin(), newAdmin);

        // 恢复后 stake 正常工作
        vm.prank(user1);
        bridge.stake(2, 100e6, bytes32(uint256(1)));

        // 旧 admin 失去权限
        vm.prank(admin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configure(address(usdc), peerContract, sourceChainId, targetChainId);
    }

    // 验证 guardian 不能重复冻结
    function testEmergencyFreeze_CannotFreezeAgain() public {
        vm.prank(guardian);
        bridge.emergencyFreeze();

        vm.prank(guardian);
        vm.expectRevert(abi.encodeWithSignature("EnforcedPause()"));
        bridge.emergencyFreeze();
    }

    // 验证 recovery 在非冻结状态下不能调用 executeRecovery
    function testRecovery_NotInEmergency() public {
        vm.prank(recovery);
        vm.expectRevert(abi.encodeWithSignature("ExpectedPause()"));
        bridge.executeRecovery(user1, address(0));
    }

    // 验证 recovery 不能指定零地址作为新 admin
    function testRecovery_ZeroAddress() public {
        vm.prank(guardian);
        bridge.emergencyFreeze();

        vm.prank(recovery);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.executeRecovery(address(0), address(0));
    }

    // 验证 recovery 清除 pendingAdmin，防止旧 admin 提议的地址在恢复后仍可接受
    function testRecovery_ClearsPendingAdmin() public {
        vm.prank(admin);
        bridge.proposeAdmin(user1);

        vm.prank(guardian);
        bridge.emergencyFreeze();

        vm.prank(recovery);
        bridge.executeRecovery(makeAddr("newAdmin"), address(0));

        // pendingAdmin 已被清除，user1 无法接受旧提议
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.acceptAdmin();
    }

    // 验证 setGuardian：admin 可设置/移除，非 admin 不能设置
    function testSetGuardian() public {
        address newGuardian = makeAddr("newGuardian");

        vm.expectEmit(true, true, false, false, address(bridge));
        emit GuardianUpdated(guardian, newGuardian);

        vm.prank(admin);
        bridge.setGuardian(newGuardian);
        assertEq(bridge.guardian(), newGuardian);

        // 新 guardian 可以冻结
        vm.prank(newGuardian);
        bridge.emergencyFreeze();

        // recovery 恢复
        vm.prank(recovery);
        bridge.executeRecovery(admin, address(0));

        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.setGuardian(address(0));

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.setGuardian(user1);
    }

    // 验证 setOperator：admin 可设置 / 移除 operator，非 admin 调用应被拒绝
    function testSetOperator() public {
        address newOper = makeAddr("newOperator");

        vm.expectEmit(true, true, false, false, address(bridge));
        emit OperatorUpdated(oper, newOper);

        vm.prank(admin);
        bridge.setOperator(newOper);
        assertEq(bridge.operator(), newOper);

        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.setOperator(address(0));

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.setOperator(user1);
    }

    // 验证紧急提取：管理员可提取全部资金，非管理员调用应被拒绝，并验证事件
    function testWithdrawToken() public {
        uint256 bridgeBal = usdc.balanceOf(address(bridge));
        uint256 adminBal = usdc.balanceOf(admin);

        vm.expectEmit(true, true, false, true, address(bridge));
        emit TokenWithdrawn(address(usdc), admin, bridgeBal);

        vm.prank(admin);
        bridge.withdrawToken(address(usdc), bridgeBal, admin);

        assertEq(usdc.balanceOf(address(bridge)), 0);
        assertEq(usdc.balanceOf(admin), adminBal + bridgeBal);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.withdrawToken(address(usdc), 1, user1);
    }

    // ========================================================================
    //                           SECURITY TESTS
    // ========================================================================

    // 验证重放攻击防护：已处理的 nonce 不能再次提交确认
    function testReplayAttack() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        _confirmToThreshold(data);
        assertTrue(_isProcessed(1));

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.AlreadyProcessed.selector);
        bridge.confirmEvent(data);
    }

    // 全面验证权限控制：非授权角色调用所有受限函数均应回退
    function testAccessControl() public {
        vm.startPrank(user1);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configure(address(usdc), peerContract, 1, 2);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.addRelayer(user2);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.removeRelayer(relayer1);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.rotateRelayer(relayer1, user2);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.proposeAdmin(user2);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.emergencyFreeze();

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.withdrawToken(address(usdc), 1, user1);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configureRateLimits(1, 1, 1, 1, 1);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.setOperator(user2);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.skipNonce(1);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.initiateRefund(1);

        // recovery 在非冻结状态下不能操作（whenPaused 先于角色检查）
        vm.expectRevert(abi.encodeWithSignature("ExpectedPause()"));
        bridge.executeRecovery(user1, address(0));

        vm.stopPrank();
    }

    // ========================================================================
    //                          QUERY FUNCTION TESTS
    // ========================================================================

    // 验证 relayers public getter 按 index 返回正确地址，且与 add/remove 同步
    function testRelayersPublicGetter() public {
        assertEq(bridge.relayers(0), relayer1);
        assertEq(bridge.relayers(1), relayer2);
        assertEq(bridge.relayers(2), relayer3);
        assertEq(bridge.getRelayerCount(), 3);

        vm.prank(admin);
        bridge.removeRelayer(relayer2);

        assertEq(bridge.relayers(0), relayer1);
        assertEq(bridge.relayers(1), relayer3);
        assertEq(bridge.getRelayerCount(), 2);
    }

    // 验证 nonceConfirmations 自动 getter 返回正确的确认进度
    function testNonceConfirmations() public {
        (bool processed, bool unlocked, uint8 threshold) = bridge.nonceConfirmations(1);
        assertFalse(processed);
        assertFalse(unlocked);
        assertEq(threshold, 0);

        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        vm.prank(relayer1);
        bridge.confirmEvent(data);

        (processed, unlocked, threshold) = bridge.nonceConfirmations(1);
        assertFalse(processed);
        assertFalse(unlocked);
        assertEq(threshold, 2);

        vm.prank(relayer2);
        bridge.confirmEvent(data);

        (processed, unlocked, threshold) = bridge.nonceConfirmations(1);
        assertTrue(processed);
        assertTrue(unlocked);
        assertEq(threshold, 2);
    }

    // ========================================================================
    //                    ADDITIONAL VALIDATION TESTS
    // ========================================================================

    // 验证 confirmEvent 拒绝 amount=0 的事件数据
    function testConfirmEvent_ZeroAmount() public {
        Bridge1024.StakeEventData memory data = _makeEventData(0, user1, 1);

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.ZeroAmount.selector);
        bridge.confirmEvent(data);
    }

    // 验证 confirmEvent 在 receiver 为 bytes32(0) 时第一个 relayer 就回退
    function testConfirmEvent_ZeroReceiver() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        data.receiver = bytes32(0);

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.confirmEvent(data);
    }

    // 验证 configureRateLimits 发出事件
    function testConfigureRateLimits_Event() public {
        vm.expectEmit(false, false, false, true, address(bridge));
        emit RateLimitsConfigured(1000e6, 7200, 500e6, 500e6, 100e6);

        vm.prank(admin);
        bridge.configureRateLimits(1000e6, 7200, 500e6, 500e6, 100e6);
    }

    // ========================================================================
    //                       SKIP NONCE & REFUND TESTS
    // ========================================================================

    // 验证 skipNonce 将 nonce 标记为已处理，后续 confirmEvent 被拒绝
    function testSkipNonce() public {
        vm.expectEmit(true, false, false, false, address(bridge));
        emit NonceSkipped(1);

        vm.prank(oper);
        bridge.skipNonce(1);

        assertTrue(_isProcessed(1));

        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.AlreadyProcessed.selector);
        bridge.confirmEvent(data);
    }

    // 验证 skipNonce 对已处理的 nonce 应 revert
    function testSkipNonce_AlreadyProcessed() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        _confirmToThreshold(data);
        assertTrue(_isProcessed(1));

        vm.prank(oper);
        vm.expectRevert(Bridge1024.AlreadyProcessed.selector);
        bridge.skipNonce(1);
    }

    // 验证非 operator 调用 skipNonce 应被拒绝
    function testSkipNonce_Unauthorized() public {
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.skipNonce(1);
    }

    // 验证 refund 正常退款流程：金额和地址从 stakes 读取，退款至原始 staker
    function testRefund() public {
        vm.prank(user1);
        bridge.stake(1, 500e6, bytes32(uint256(0xdeadbeef)));

        (address owner, uint64 amount, bool refunded) = bridge.stakes(1);
        assertEq(amount, 500e6);
        assertEq(owner, user1);
        assertFalse(refunded);

        vm.prank(oper);
        bridge.initiateRefund(1);

        vm.warp(block.timestamp + 6 hours);

        uint256 userBalBefore = usdc.balanceOf(user1);
        uint256 bridgeBalBefore = usdc.balanceOf(address(bridge));

        vm.expectEmit(true, true, false, true, address(bridge));
        emit Refunded(1, user1, 500e6);

        vm.prank(oper);
        bridge.executeRefund(1);

        assertEq(usdc.balanceOf(user1), userBalBefore + 500e6);
        assertEq(usdc.balanceOf(address(bridge)), bridgeBalBefore - 500e6);
        (, , bool isRefunded) = bridge.stakes(1);
        assertTrue(isRefunded);
    }

    // 验证 refund 始终退回给原始 staker，operator 无法改变退款地址
    function testRefund_AlwaysToStaker() public {
        vm.prank(user1);
        bridge.stake(1, 300e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(1);

        vm.warp(block.timestamp + 6 hours);

        uint256 user1BalBefore = usdc.balanceOf(user1);
        uint256 user2BalBefore = usdc.balanceOf(user2);

        vm.prank(oper);
        bridge.executeRefund(1);

        assertEq(usdc.balanceOf(user1), user1BalBefore + 300e6, "refund should go to original staker");
        assertEq(usdc.balanceOf(user2), user2BalBefore, "user2 should not receive anything");
    }

    // 验证重复退款应 revert
    function testRefund_AlreadyRefunded() public {
        vm.prank(user1);
        bridge.stake(1, 500e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(1);

        vm.warp(block.timestamp + 6 hours);

        vm.prank(oper);
        bridge.executeRefund(1);

        vm.prank(oper);
        vm.expectRevert(Bridge1024.AlreadyRefunded.selector);
        bridge.initiateRefund(1);
    }

    // 验证未 stake 的 nonce（stakes.owner 为零地址）应 revert
    function testRefund_InvalidParams() public {
        vm.prank(oper);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.initiateRefund(1);

        vm.prank(oper);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.initiateRefund(99);
    }

    // 验证非 operator 调用 refund 应被拒绝
    function testRefund_Unauthorized() public {
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.initiateRefund(1);
    }

    // ========================================================================
    //                         TIMELOCK TESTS
    // ========================================================================

    // 验证 activateTimelock：激活成功、事件触发、不可重复激活、非管理员不可激活
    function testActivateTimelock() public {
        assertFalse(bridge.timelockActive());

        vm.expectEmit(false, false, false, false, address(bridge));
        emit TimelockActivated();

        vm.prank(admin);
        bridge.activateTimelock();
        assertTrue(bridge.timelockActive());

        vm.prank(admin);
        vm.expectRevert(Bridge1024.TimelockAlreadyActive.selector);
        bridge.activateTimelock();

        vm.prank(admin);
        Bridge1024 freshBridge = new Bridge1024(guardian, oper, recovery);
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        freshBridge.activateTimelock();
    }

    // 验证时间锁未激活时 scheduleOperation 应 revert
    function testSchedule_BeforeActivation() public {
        bytes memory data = abi.encode("configure", address(usdc), peerContract, uint64(1), uint64(2));
        vm.prank(admin);
        vm.expectRevert(Bridge1024.TimelockNotActive.selector);
        bridge.scheduleOperation(data);
    }

    // 验证时间锁激活后 configure 完整流程：调度 → 等待 → 执行
    function testTimelock_ConfigureFlow() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        MockUSDC newUsdc = new MockUSDC();
        bytes32 newPeer = bytes32(uint256(0xabcdef));
        bytes memory data = abi.encode("configure", address(newUsdc), newPeer, uint64(10), uint64(20));
        bytes32 opHash = keccak256(data);

        vm.expectEmit(true, false, false, true, address(bridge));
        emit OperationScheduled(opHash, uint64(block.timestamp) + 24 hours, data);
        bridge.scheduleOperation(data);

        assertEq(bridge.timelockEta(opHash), uint64(block.timestamp) + 24 hours);

        vm.expectRevert(Bridge1024.TimelockNotReady.selector);
        bridge.configure(address(newUsdc), newPeer, 10, 20);

        vm.warp(block.timestamp + 24 hours);

        vm.expectEmit(true, false, false, false, address(bridge));
        emit OperationExecuted(opHash);
        bridge.configure(address(newUsdc), newPeer, 10, 20);

        assertEq(bridge.usdcContract(), address(newUsdc));
        assertEq(bridge.peerContract(), newPeer);
        assertEq(bridge.timelockEta(opHash), 0);
        vm.stopPrank();
    }

    // 验证时间锁激活后，未调度直接执行应 revert
    function testTimelock_ConfigureWithoutSchedule() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        vm.expectRevert(Bridge1024.TimelockNotScheduled.selector);
        bridge.configure(address(usdc), peerContract, 1, 2);
        vm.stopPrank();
    }

    // 验证取消已调度操作后再执行应 revert
    function testTimelock_CancelOperation() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        bytes memory data = abi.encode("configure", address(usdc), peerContract, uint64(1), uint64(2));
        bytes32 opHash = keccak256(data);

        bridge.scheduleOperation(data);

        vm.expectEmit(true, false, false, false, address(bridge));
        emit OperationCancelled(opHash);
        bridge.cancelOperation(opHash);
        assertEq(bridge.timelockEta(opHash), 0);

        vm.warp(block.timestamp + 24 hours);
        vm.expectRevert(Bridge1024.TimelockNotScheduled.selector);
        bridge.configure(address(usdc), peerContract, 1, 2);
        vm.stopPrank();
    }

    // 验证取消未调度的操作应 revert
    function testTimelock_CancelNotScheduled() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        vm.expectRevert(Bridge1024.TimelockNotScheduled.selector);
        bridge.cancelOperation(bytes32(uint256(0x1234)));
        vm.stopPrank();
    }

    // 验证重复调度同一操作应 revert
    function testTimelock_DuplicateSchedule() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        bytes memory data = abi.encode("configure", address(usdc), peerContract, uint64(1), uint64(2));
        bridge.scheduleOperation(data);

        vm.expectRevert(Bridge1024.TimelockAlreadyScheduled.selector);
        bridge.scheduleOperation(data);
        vm.stopPrank();
    }

    // 验证 configureRateLimits 的时间锁流程
    function testTimelock_ConfigureRateLimitsFlow() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        bytes memory data = abi.encode("configureRateLimits", uint64(1000e6), uint64(7200), uint64(500e6), uint64(500e6), uint64(100e6));
        bridge.scheduleOperation(data);

        vm.warp(block.timestamp + 24 hours);
        bridge.configureRateLimits(1000e6, 7200, 500e6, 500e6, 100e6);

        assertEq(bridge.maxUnlockPerWindow(), 1000e6);
        assertEq(bridge.windowDuration(), 7200);
        assertEq(bridge.maxSingleUnlock(), 500e6);
        assertEq(bridge.maxStakeAmount(), 500e6);
        assertEq(bridge.minimumReserve(), 100e6);
        vm.stopPrank();
    }

    // 验证 withdrawToken 的时间锁流程
    function testTimelock_WithdrawTokenFlow() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        uint256 amount = usdc.balanceOf(address(bridge));
        bytes memory data = abi.encode("withdrawToken", address(usdc), amount, admin);
        bridge.scheduleOperation(data);

        vm.warp(block.timestamp + 24 hours);
        bridge.withdrawToken(address(usdc), amount, admin);

        assertEq(usdc.balanceOf(address(bridge)), 0);
        vm.stopPrank();
    }

    // 验证 addRelayer 的时间锁流程
    function testTimelock_AddRelayerFlow() public {
        address newRelayer = makeAddr("timelockRelayer");

        vm.startPrank(admin);
        bridge.activateTimelock();

        bytes memory data = abi.encode("addRelayer", newRelayer);
        bridge.scheduleOperation(data);

        vm.expectRevert(Bridge1024.TimelockNotReady.selector);
        bridge.addRelayer(newRelayer);

        vm.warp(block.timestamp + 24 hours);
        bridge.addRelayer(newRelayer);

        assertTrue(bridge.isRelayer(newRelayer));
        vm.stopPrank();
    }

    // 验证 removeRelayer 的时间锁流程
    function testTimelock_RemoveRelayerFlow() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        bytes memory data = abi.encode("removeRelayer", relayer3);
        bridge.scheduleOperation(data);

        vm.warp(block.timestamp + 24 hours);
        bridge.removeRelayer(relayer3);

        assertFalse(bridge.isRelayer(relayer3));
        vm.stopPrank();
    }

    // 验证 rotateRelayer 的时间锁流程
    function testTimelock_RotateRelayerFlow() public {
        address newRelayer = makeAddr("rotatedTimelock");

        vm.startPrank(admin);
        bridge.activateTimelock();

        bytes memory data = abi.encode("rotateRelayer", relayer2, newRelayer);
        bridge.scheduleOperation(data);

        vm.warp(block.timestamp + 24 hours);
        bridge.rotateRelayer(relayer2, newRelayer);

        assertFalse(bridge.isRelayer(relayer2));
        assertTrue(bridge.isRelayer(newRelayer));
        vm.stopPrank();
    }

    // 验证 proposeAdmin 的时间锁流程
    function testTimelock_ProposeAdminFlow() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        // 未调度直接执行应 revert
        vm.expectRevert(Bridge1024.TimelockNotScheduled.selector);
        bridge.proposeAdmin(user1);

        bytes memory data = abi.encode("proposeAdmin", user1);
        bridge.scheduleOperation(data);

        // 延迟未到应 revert
        vm.expectRevert(Bridge1024.TimelockNotReady.selector);
        bridge.proposeAdmin(user1);

        vm.warp(block.timestamp + 24 hours);
        bridge.proposeAdmin(user1);
        vm.stopPrank();

        // 被提议人可正常接受
        vm.prank(user1);
        bridge.acceptAdmin();

        assertEq(bridge.admin(), user1);
    }

    // 验证时间锁未激活时所有受保护函数仍可直接调用（初始部署场景）
    function testTimelock_BypassBeforeActivation() public {
        vm.prank(admin);
        Bridge1024 freshBridge = new Bridge1024(guardian, oper, recovery);
        MockUSDC freshUsdc = new MockUSDC();

        vm.startPrank(admin);
        freshBridge.configure(address(freshUsdc), peerContract, sourceChainId, targetChainId);
        freshBridge.configureRateLimits(type(uint64).max, 3600, type(uint64).max, 0, 0);
        freshBridge.addRelayer(relayer1);
        freshBridge.addRelayer(relayer2);
        freshBridge.addRelayer(relayer3);
        vm.stopPrank();

        assertEq(freshBridge.usdcContract(), address(freshUsdc));
        assertEq(freshBridge.getRelayerCount(), 3);
        assertFalse(freshBridge.timelockActive());
    }

    // 验证非管理员不能调度、取消操作
    function testTimelock_Unauthorized() public {
        vm.prank(admin);
        bridge.activateTimelock();

        bytes memory data = abi.encode("configure", address(usdc), peerContract, uint64(1), uint64(2));
        bytes32 opHash = keccak256(data);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.scheduleOperation(data);

        vm.prank(admin);
        bridge.scheduleOperation(data);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.cancelOperation(opHash);
    }

    // ========================================================================
    //                    AUDIT FIX TESTS (H-1, M-3, L-1, L-3)
    // ========================================================================

    // H-1: 验证 configure 拒绝零值 peerContract
    function testConfigure_ZeroPeerContract() public {
        MockUSDC newUsdc = new MockUSDC();
        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.configure(address(newUsdc), bytes32(0), 10, 20);
    }

    // H-1: 验证 configure 拒绝零值 localChainId
    function testConfigure_ZeroLocalChainId() public {
        MockUSDC newUsdc = new MockUSDC();
        bytes32 peer = bytes32(uint256(0xabcdef));
        vm.prank(admin);
        vm.expectRevert(Bridge1024.InvalidChainId.selector);
        bridge.configure(address(newUsdc), peer, 0, 20);
    }

    // H-1: 验证 configure 拒绝零值 peerChainId
    function testConfigure_ZeroPeerChainId() public {
        MockUSDC newUsdc = new MockUSDC();
        bytes32 peer = bytes32(uint256(0xabcdef));
        vm.prank(admin);
        vm.expectRevert(Bridge1024.InvalidChainId.selector);
        bridge.configure(address(newUsdc), peer, 10, 0);
    }

    // H-1: 验证 configure 拒绝相同的 localChainId 和 peerChainId（自环）
    function testConfigure_SameChainIds() public {
        MockUSDC newUsdc = new MockUSDC();
        bytes32 peer = bytes32(uint256(0xabcdef));
        vm.prank(admin);
        vm.expectRevert(Bridge1024.InvalidChainId.selector);
        bridge.configure(address(newUsdc), peer, 10, 10);
    }

    // M-3: 验证 confirmEvent 在 receiver 高位非零时第一个 relayer 就 revert
    function testConfirmEvent_InvalidReceiverHighBits() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        data.receiver = bytes32(uint256(uint160(user1)) | (uint256(0xFF) << 160));

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidReceiver.selector);
        bridge.confirmEvent(data);
    }

    // L-3: 验证合约显式拒绝直接 ETH 转账
    function testReceive_RejectsETH() public {
        vm.deal(user1, 1 ether);
        vm.prank(user1);
        (bool success, ) = address(bridge).call{value: 1 ether}("");
        assertFalse(success);
    }

    // L-3: 验证 withdrawETH 可提取被强制发送的 ETH
    function testWithdrawETH() public {
        vm.deal(address(bridge), 1 ether);

        uint256 adminBalBefore = admin.balance;

        vm.expectEmit(true, false, false, true, address(bridge));
        emit ETHWithdrawn(admin, 1 ether);

        vm.prank(admin);
        bridge.withdrawETH(payable(admin));

        assertEq(address(bridge).balance, 0);
        assertEq(admin.balance, adminBalBefore + 1 ether);
    }

    // L-3: 验证 withdrawETH 零余额和零地址 revert
    function testWithdrawETH_InvalidParams() public {
        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAmount.selector);
        bridge.withdrawETH(payable(admin));

        vm.deal(address(bridge), 1 ether);
        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.withdrawETH(payable(address(0)));
    }

    // L-3: 验证非管理员不能调用 withdrawETH
    function testWithdrawETH_Unauthorized() public {
        vm.deal(address(bridge), 1 ether);
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.withdrawETH(payable(user1));
    }

    // ========================================================================
    //              AUDIT FIX TESTS (H-5, H-6, M-4, L-4, L-5, L-6)
    // ========================================================================

    // H-5: 验证 setOperator 在 timelock 激活后需要走时间锁流程
    function testTimelock_SetOperatorFlow() public {
        address newOper = makeAddr("timelockOper");

        vm.startPrank(admin);
        bridge.activateTimelock();

        vm.expectRevert(Bridge1024.TimelockNotScheduled.selector);
        bridge.setOperator(newOper);

        bytes memory data = abi.encode("setOperator", newOper);
        bridge.scheduleOperation(data);

        vm.warp(block.timestamp + 24 hours);
        bridge.setOperator(newOper);

        assertEq(bridge.operator(), newOper);
        vm.stopPrank();
    }

    // H-6: 验证 refund 受速率限制约束
    function testRefund_RateLimited() public {
        vm.prank(admin);
        bridge.configureRateLimits(400e6, 3600, 0, 0, 0);

        vm.prank(user1);
        bridge.stake(1, 500e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(1);

        vm.warp(block.timestamp + 6 hours);

        vm.prank(oper);
        vm.expectRevert(Bridge1024.RateLimitExceeded.selector);
        bridge.executeRefund(1);
    }

    // H-6: 验证 refund 受储备金约束
    function testRefund_InsufficientReserve() public {
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, 0, 0, 100_500e6);

        vm.prank(user1);
        bridge.stake(1, 500e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(1);

        vm.warp(block.timestamp + 6 hours);

        vm.prank(oper);
        vm.expectRevert(Bridge1024.InsufficientReserve.selector);
        bridge.executeRefund(1);
    }

    // M-4: 验证操作超过 grace period 后过期不可执行
    function testTimelock_Expired() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        MockUSDC newUsdc = new MockUSDC();
        bytes32 newPeer = bytes32(uint256(0xabcdef));
        bytes memory data = abi.encode("configure", address(newUsdc), newPeer, uint64(10), uint64(20));
        bridge.scheduleOperation(data);

        vm.warp(block.timestamp + 24 hours + 48 hours + 1);

        vm.expectRevert(Bridge1024.TimelockExpired.selector);
        bridge.configure(address(newUsdc), newPeer, 10, 20);
        vm.stopPrank();
    }

    // M-4: 验证在 grace period 最后一秒仍可执行
    function testTimelock_ExecuteAtGracePeriodBoundary() public {
        vm.startPrank(admin);
        bridge.activateTimelock();

        MockUSDC newUsdc = new MockUSDC();
        bytes32 newPeer = bytes32(uint256(0xabcdef));
        bytes memory data = abi.encode("configure", address(newUsdc), newPeer, uint64(10), uint64(20));
        bridge.scheduleOperation(data);

        vm.warp(block.timestamp + 24 hours + 48 hours);
        bridge.configure(address(newUsdc), newPeer, 10, 20);

        assertEq(bridge.usdcContract(), address(newUsdc));
        vm.stopPrank();
    }

    // L-5: 验证 setGuardian 在 timelock 激活后需要走时间锁流程
    function testTimelock_SetGuardianFlow() public {
        address newGuardian = makeAddr("timelockGuardian");

        vm.startPrank(admin);
        bridge.activateTimelock();

        vm.expectRevert(Bridge1024.TimelockNotScheduled.selector);
        bridge.setGuardian(newGuardian);

        bytes memory data = abi.encode("setGuardian", newGuardian);
        bridge.scheduleOperation(data);

        vm.warp(block.timestamp + 24 hours);
        bridge.setGuardian(newGuardian);

        assertEq(bridge.guardian(), newGuardian);
        vm.stopPrank();
    }

    // L-6: 验证 configureRateLimits 拒绝不合理的参数组合
    function testConfigureRateLimits_InvalidParams() public {
        vm.startPrank(admin);

        // maxPerWindow 非零但 windowDuration 为零（不一致）
        vm.expectRevert(Bridge1024.InvalidRateLimitParams.selector);
        bridge.configureRateLimits(500e6, 0, 0, 0, 0);

        // maxPerWindow 为零但 windowDuration 非零（不一致）
        vm.expectRevert(Bridge1024.InvalidRateLimitParams.selector);
        bridge.configureRateLimits(0, 3600, 0, 0, 0);

        // maxSingle > maxPerWindow（都非零时矛盾）
        vm.expectRevert(Bridge1024.InvalidRateLimitParams.selector);
        bridge.configureRateLimits(500e6, 3600, 1000e6, 0, 0);

        // windowDuration 太小（< 60 秒）
        vm.expectRevert(Bridge1024.InvalidRateLimitParams.selector);
        bridge.configureRateLimits(500e6, 30, 200e6, 0, 0);

        // 合法：maxSingle = 0 表示不限单笔
        bridge.configureRateLimits(500e6, 3600, 0, 0, 0);

        // 合法：全部为 0 表示关闭限制
        bridge.configureRateLimits(0, 0, 0, 0, 0);

        vm.stopPrank();
    }

    // ========================================================================
    //                     STAKE AMOUNT LIMIT TESTS
    // ========================================================================

    // 验证 stake 金额超过 maxStakeAmount 时应回退
    function testStake_ExceedsMaxStakeAmount() public {
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, type(uint64).max, 500e6, 0);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.StakeAmountExceeded.selector);
        bridge.stake(1, 600e6, bytes32(uint256(0xdeadbeef)));
    }

    // 验证 stake 金额等于 maxStakeAmount 时可正常通过
    function testStake_AtMaxStakeAmount() public {
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, type(uint64).max, 500e6, 0);

        vm.prank(user1);
        bridge.stake(1, 500e6, bytes32(uint256(0xdeadbeef)));
    }

    // 验证 maxStakeAmount = 0 时不限制（默认行为）
    function testStake_NoLimitWhenMaxStakeZero() public {
        vm.prank(user1);
        bridge.stake(1, 5000e6, bytes32(uint256(0xdeadbeef)));
        (, uint64 staked,) = bridge.stakes(1);
        assertEq(staked, 5000e6);
    }

    // INV-R5-1: 不变量"能 stake 必须能 refund"——
    // 即便事后管理员把 maxSingleUnlock 调到比已有 stake 还小，已发生的 refund 也必须畅通，
    // 否则用户资金会被永久卡死。该不变量使得 maxSingleUnlock 仅作用于跨链 unlock 路径，
    // 不再波及 refund（与 confirmEvent 入口的早拒分工明确）
    function testRefund_NotBlockedByMaxSingleUnlock() public {
        vm.prank(user1);
        bridge.stake(1, 1000e6, bytes32(uint256(0xdeadbeef)));

        // 事后把 maxSingleUnlock 收紧到 500e6（小于已发生 stake 的 1000e6）
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, 500e6, 0, 0);

        vm.prank(oper);
        bridge.initiateRefund(1);

        vm.warp(block.timestamp + 6 hours);

        uint256 balBefore = usdc.balanceOf(user1);

        // refund 必须成功，不再被 maxSingleUnlock 卡住
        vm.prank(oper);
        bridge.executeRefund(1);

        // 用户拿到全额本金
        assertEq(usdc.balanceOf(user1), balBefore + 1000e6);
    }

    // INV-R5-1（镜像）: 跨链入金路径仍然必须受 maxSingleUnlock 约束
    function testConfirmEvent_StillBoundByMaxSingleUnlock() public {
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, 500e6, 0, 0);

        Bridge1024.StakeEventData memory data = _makeEventData(1000e6, user1, 50);
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.SingleTransferExceeded.selector);
        bridge.confirmEvent(data);
    }

    // ========================================================================
    //              EMERGENCY FREEZE & RECOVERY TESTS
    // ========================================================================

    // 验证 setRecovery 需要 timelock（激活后）
    function testTimelock_SetRecoveryFlow() public {
        address newRecovery = makeAddr("newRecovery");

        vm.startPrank(admin);
        bridge.activateTimelock();

        vm.expectRevert(Bridge1024.TimelockNotScheduled.selector);
        bridge.setRecovery(newRecovery);

        bytes memory data = abi.encode("setRecovery", newRecovery);
        bridge.scheduleOperation(data);

        vm.warp(block.timestamp + 24 hours);

        vm.expectEmit(true, true, false, false, address(bridge));
        emit RecoveryUpdated(recovery, newRecovery);
        bridge.setRecovery(newRecovery);

        assertEq(bridge.recovery(), newRecovery);
        vm.stopPrank();
    }

    // 验证 setRecovery 拒绝零地址
    function testSetRecovery_ZeroAddress() public {
        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.setRecovery(address(0));
    }

    // 验证非管理员不能设置 recovery
    function testSetRecovery_Unauthorized() public {
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.setRecovery(user1);
    }

    // 验证 timelock 未激活时 setRecovery 可直接调用
    function testSetRecovery_BeforeTimelock() public {
        address newRecovery = makeAddr("newRecovery");

        vm.prank(admin);
        bridge.setRecovery(newRecovery);
        assertEq(bridge.recovery(), newRecovery);
    }

    // 验证构造函数拒绝任何角色地址为零
    function testConstructor_ZeroAddress() public {
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        new Bridge1024(address(0), oper, recovery);

        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        new Bridge1024(guardian, address(0), recovery);

        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        new Bridge1024(guardian, oper, address(0));
    }

    // 综合场景：admin 密钥泄露后的完整攻防时间线
    function testEmergencyFreeze_AttackScenario() public {
        vm.prank(admin);
        bridge.activateTimelock();

        // 攻击者用泄露的 admin 密钥调度 withdrawToken
        uint256 vaultBalance = usdc.balanceOf(address(bridge));
        bytes memory maliciousOp = abi.encode("withdrawToken", address(usdc), vaultBalance, admin);
        vm.prank(admin);
        bridge.scheduleOperation(maliciousOp);

        // Guardian 检测到异常调度，紧急冻结
        vm.prank(guardian);
        bridge.emergencyFreeze();

        // 24h 后攻击者尝试执行 withdrawToken → 被 EnforcedPause 阻止
        vm.warp(block.timestamp + 24 hours);
        vm.prank(admin);
        vm.expectRevert(abi.encodeWithSignature("EnforcedPause()"));
        bridge.withdrawToken(address(usdc), vaultBalance, admin);

        // Recovery 介入，设置新 admin
        address newAdmin = makeAddr("newAdmin");
        vm.prank(recovery);
        bridge.executeRecovery(newAdmin, address(0));

        // 新 admin 取消攻击者遗留的调度操作
        vm.prank(newAdmin);
        bridge.cancelOperation(keccak256(maliciousOp));

        // 资金安全
        assertEq(usdc.balanceOf(address(bridge)), vaultBalance);
    }

    // M-NEW-2: 验证 executeRecovery 可同时替换 guardian，打破恶意 guardian DoS 循环
    function testRecovery_WithNewGuardian() public {
        vm.prank(guardian);
        bridge.emergencyFreeze();

        address newAdmin = makeAddr("newAdmin");
        address newGuardian = makeAddr("newGuardian");

        vm.expectEmit(true, true, false, false, address(bridge));
        emit GuardianUpdated(guardian, newGuardian);
        vm.expectEmit(true, true, false, false, address(bridge));
        emit RecoveryExecuted(admin, newAdmin);

        vm.prank(recovery);
        bridge.executeRecovery(newAdmin, newGuardian);

        assertFalse(bridge.paused());
        assertEq(bridge.admin(), newAdmin);
        assertEq(bridge.guardian(), newGuardian);

        // 旧 guardian 不能冻结
        vm.prank(guardian);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.emergencyFreeze();

        // 新 guardian 可以冻结
        vm.prank(newGuardian);
        bridge.emergencyFreeze();
        assertTrue(bridge.paused());
    }

    // M-NEW-2: 验证 executeRecovery 传 address(0) 保留当前 guardian
    function testRecovery_KeepGuardian() public {
        vm.prank(guardian);
        bridge.emergencyFreeze();

        vm.prank(recovery);
        bridge.executeRecovery(makeAddr("newAdmin"), address(0));

        assertEq(bridge.guardian(), guardian);
    }

    // L-NEW-1: 验证构造函数拒绝角色地址重叠
    function testConstructor_RoleOverlap() public {
        // admin (msg.sender) == guardian
        vm.prank(guardian);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        new Bridge1024(guardian, oper, recovery);

        // admin (msg.sender) == operator
        vm.prank(oper);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        new Bridge1024(guardian, oper, recovery);

        // admin (msg.sender) == recovery
        vm.prank(recovery);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        new Bridge1024(guardian, oper, recovery);

        // guardian == operator
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        new Bridge1024(guardian, guardian, recovery);

        // guardian == recovery
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        new Bridge1024(guardian, oper, guardian);

        // operator == recovery
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        new Bridge1024(guardian, oper, oper);
    }

    // L-NEW-4: 验证 cancelOperation 在暂停状态下仍可调用
    function testCancelOperation_WhenPaused() public {
        vm.prank(admin);
        bridge.activateTimelock();

        bytes memory data = abi.encode("configure", address(usdc), peerContract, uint64(1), uint64(2));
        bytes32 opHash = keccak256(data);

        vm.prank(admin);
        bridge.scheduleOperation(data);

        // Guardian 冻结合约
        vm.prank(guardian);
        bridge.emergencyFreeze();
        assertTrue(bridge.paused());

        // Recovery 恢复
        address newAdmin = makeAddr("newAdmin");
        vm.prank(recovery);
        bridge.executeRecovery(newAdmin, address(0));

        // 恶意 guardian 立即再次冻结
        vm.prank(guardian);
        bridge.emergencyFreeze();
        assertTrue(bridge.paused());

        // 新 admin 在暂停状态下仍可取消遗留操作
        vm.prank(newAdmin);
        bridge.cancelOperation(opHash);
        assertEq(bridge.timelockEta(opHash), 0);
    }

    // ========================================================================
    //          AUDIT FIX TESTS — R4: ROLE OVERLAP ON MUTATION
    // ========================================================================

    // M-R4-1: setGuardian 拒绝与 admin/operator/recovery 重叠
    function testSetGuardian_RoleOverlap() public {
        vm.startPrank(admin);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setGuardian(admin);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setGuardian(oper);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setGuardian(recovery);

        // 合法地址可通过
        address newGuardian = makeAddr("newGuardian2");
        bridge.setGuardian(newGuardian);
        assertEq(bridge.guardian(), newGuardian);
        vm.stopPrank();
    }

    // M-R4-1: setOperator 拒绝与 admin/guardian/recovery 重叠
    function testSetOperator_RoleOverlap() public {
        vm.startPrank(admin);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setOperator(admin);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setOperator(guardian);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setOperator(recovery);

        address newOper = makeAddr("newOper2");
        bridge.setOperator(newOper);
        assertEq(bridge.operator(), newOper);
        vm.stopPrank();
    }

    // M-R4-1: setRecovery 拒绝与 admin/guardian/operator 重叠
    function testSetRecovery_RoleOverlap() public {
        vm.startPrank(admin);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setRecovery(admin);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setRecovery(guardian);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setRecovery(oper);

        address newRecovery = makeAddr("newRecovery2");
        bridge.setRecovery(newRecovery);
        assertEq(bridge.recovery(), newRecovery);
        vm.stopPrank();
    }

    // M-R5-1: proposeAdmin 提前拒绝与 admin/guardian/operator/recovery 重叠的提议，
    // 避免 timelock 调度被白白消耗、以及 pendingAdmin 卡死在 acceptAdmin 阶段
    function testProposeAdmin_RoleOverlap() public {
        vm.startPrank(admin);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.proposeAdmin(admin);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.proposeAdmin(guardian);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.proposeAdmin(oper);

        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.proposeAdmin(recovery);

        // 合法地址可正常提议并接受
        address newAdmin = makeAddr("newAdmin2");
        bridge.proposeAdmin(newAdmin);
        vm.stopPrank();

        vm.prank(newAdmin);
        bridge.acceptAdmin();

        assertEq(bridge.admin(), newAdmin);
    }

    // M-R5-1（深度防御）: acceptAdmin 中的 RoleOverlap 已由 propose/set 的预检阻断到达，
    // 此处保留 acceptAdmin 在合法路径上的行为校验，确保流程依旧畅通
    function testAcceptAdmin_RoleOverlap() public {
        address newAdmin = makeAddr("newAdmin2");
        vm.prank(admin);
        bridge.proposeAdmin(newAdmin);

        vm.prank(newAdmin);
        bridge.acceptAdmin();

        assertEq(bridge.admin(), newAdmin);
    }

    // M-R5-1: setGuardian 拒绝与 pendingAdmin 重叠（否则会让 acceptAdmin 永久卡死）
    function testSetGuardian_RoleOverlap_PendingAdmin() public {
        address newAdmin = makeAddr("pendingAdmin1");
        vm.prank(admin);
        bridge.proposeAdmin(newAdmin);

        vm.prank(admin);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setGuardian(newAdmin);
    }

    // M-R5-1: setOperator 拒绝与 pendingAdmin 重叠
    function testSetOperator_RoleOverlap_PendingAdmin() public {
        address newAdmin = makeAddr("pendingAdmin2");
        vm.prank(admin);
        bridge.proposeAdmin(newAdmin);

        vm.prank(admin);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setOperator(newAdmin);
    }

    // M-R5-1: setRecovery 拒绝与 pendingAdmin 重叠
    function testSetRecovery_RoleOverlap_PendingAdmin() public {
        address newAdmin = makeAddr("pendingAdmin3");
        vm.prank(admin);
        bridge.proposeAdmin(newAdmin);

        vm.prank(admin);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.setRecovery(newAdmin);
    }

    // M-R4-1: executeRecovery 拒绝 newAdmin 与其他角色重叠
    function testRecovery_RoleOverlap_NewAdmin() public {
        vm.prank(guardian);
        bridge.emergencyFreeze();

        // newAdmin = operator → revert
        vm.prank(recovery);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.executeRecovery(oper, address(0));

        // newAdmin = recovery → revert
        vm.prank(recovery);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.executeRecovery(recovery, address(0));

        // newAdmin = guardian（保留 guardian 时重叠） → revert
        vm.prank(recovery);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.executeRecovery(guardian, address(0));
    }

    // M-R4-1: executeRecovery 拒绝 newGuardian 与其他角色重叠
    function testRecovery_RoleOverlap_NewGuardian() public {
        vm.prank(guardian);
        bridge.emergencyFreeze();

        address newAdmin = makeAddr("newAdmin3");

        // newGuardian = operator → revert
        vm.prank(recovery);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.executeRecovery(newAdmin, oper);

        // newGuardian = recovery → revert
        vm.prank(recovery);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.executeRecovery(newAdmin, recovery);

        // newGuardian = newAdmin → revert
        vm.prank(recovery);
        vm.expectRevert(Bridge1024.RoleOverlap.selector);
        bridge.executeRecovery(newAdmin, newAdmin);

        // 合法组合可通过
        address newGuardian = makeAddr("newGuardian3");
        vm.prank(recovery);
        bridge.executeRecovery(newAdmin, newGuardian);

        assertEq(bridge.admin(), newAdmin);
        assertEq(bridge.guardian(), newGuardian);
    }

    // ========================================================================
    //                    RANDOM NONCE & TWO-STEP REFUND TESTS
    // ========================================================================

    // 验证随机 nonce 防碰撞：同一 nonce 第二次 stake 应回退
    function testStake_NonceAlreadyUsed() public {
        vm.prank(user1);
        bridge.stake(42, 100e6, bytes32(uint256(1)));

        vm.prank(user2);
        vm.expectRevert(Bridge1024.NonceAlreadyUsed.selector);
        bridge.stake(42, 200e6, bytes32(uint256(2)));
    }

    // 验证原始 staker 可以执行退款第二步
    function testRefund_StakerCanExecute() public {
        vm.prank(user1);
        bridge.stake(55, 500e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(55);

        vm.warp(block.timestamp + 6 hours);

        uint256 userBalBefore = usdc.balanceOf(user1);

        vm.prank(user1);
        bridge.executeRefund(55);

        assertEq(usdc.balanceOf(user1), userBalBefore + 500e6);
        (, , bool isRefunded) = bridge.stakes(55);
        assertTrue(isRefunded);
    }

    // 验证非 operator 且非 staker 不能执行退款
    function testRefund_UnauthorizedExecute() public {
        vm.prank(user1);
        bridge.stake(56, 500e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(56);

        vm.warp(block.timestamp + 6 hours);

        vm.prank(user2);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.executeRefund(56);
    }

    // 验证 admin 可以取消已发起的退款
    function testCancelRefund() public {
        vm.prank(user1);
        bridge.stake(57, 500e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(57);

        vm.expectEmit(true, false, false, false, address(bridge));
        emit RefundCancelled(57);

        vm.prank(admin);
        bridge.cancelRefund(57);

        vm.warp(block.timestamp + 6 hours);

        vm.prank(oper);
        vm.expectRevert(Bridge1024.RefundNotInitiated.selector);
        bridge.executeRefund(57);
    }

    // 验证取消未发起的退款应 revert
    function testCancelRefund_NotInitiated() public {
        vm.prank(admin);
        vm.expectRevert(Bridge1024.RefundNotInitiated.selector);
        bridge.cancelRefund(99);
    }

    // 验证延迟时间未到时执行退款应 revert
    function testRefund_DelayNotElapsed() public {
        vm.prank(user1);
        bridge.stake(58, 500e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(58);

        vm.warp(block.timestamp + 5 hours);

        vm.prank(oper);
        vm.expectRevert(Bridge1024.RefundNotReady.selector);
        bridge.executeRefund(58);
    }

    // 验证重复发起退款应 revert
    function testRefund_AlreadyInitiated() public {
        vm.prank(user1);
        bridge.stake(59, 500e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(59);

        vm.prank(oper);
        vm.expectRevert(Bridge1024.RefundAlreadyInitiated.selector);
        bridge.initiateRefund(59);
    }

    // 验证 cancelRefund 在暂停状态下仍可调用
    function testCancelRefund_WhenPaused() public {
        vm.prank(user1);
        bridge.stake(60, 500e6, bytes32(uint256(0xdeadbeef)));

        vm.prank(oper);
        bridge.initiateRefund(60);

        vm.prank(guardian);
        bridge.emergencyFreeze();

        vm.prank(admin);
        bridge.cancelRefund(60);

        assertEq(bridge.refundInitiatedAt(60), 0);
    }

    // L-OLD-1: 验证 configureRateLimits 重置窗口状态，降低限额后不会卡死
    function testConfigureRateLimits_ResetsWindow() public {
        vm.prank(admin);
        bridge.configureRateLimits(1000e6, 3600, 0, 0, 0);

        // 在当前窗口内用掉 800e6 额度
        Bridge1024.StakeEventData memory data1 = _makeEventData(800e6, user1, 1);
        _confirmToThreshold(data1);
        assertTrue(_isProcessed(1));

        // 将限额从 1000e6 降到 500e6 — 旧用量 (800e6) 已超过新限额
        // 如果不重置窗口，后续所有 unlock 都会被 RateLimitExceeded 阻塞
        vm.prank(admin);
        bridge.configureRateLimits(500e6, 3600, 0, 0, 0);

        // 重置后 400e6 应该可以正常通过（在新窗口的 500e6 限额内）
        Bridge1024.StakeEventData memory data2 = _makeEventData(400e6, user1, 2);
        _confirmToThreshold(data2);
        assertTrue(_isProcessed(2));
    }

    // ========================================================================
    //                           VIEW FUNCTION TESTS
    // ========================================================================

    function testGetBridgeInfo() public view {
        (
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
        ) = bridge.getBridgeInfo();

        assertEq(_admin, admin);
        assertEq(_guardian, guardian);
        assertEq(_operator, oper);
        assertEq(_recovery, recovery);
        assertEq(_pendingAdmin, address(0));
        assertEq(_usdcContract, address(usdc));
        assertEq(_peerContract, peerContract);
        assertEq(_localChainId, sourceChainId);
        assertEq(_peerChainId, targetChainId);
        assertFalse(_paused);
        assertFalse(_timelockActive);
        assertEq(_relayerCount, 3);
    }

    function testGetBridgeInfo_ReflectsStateChanges() public {
        // Freeze → paused should be true
        vm.prank(guardian);
        bridge.emergencyFreeze();

        (, , , , , , , , , bool _paused, , ) = bridge.getBridgeInfo();
        assertTrue(_paused);

        // Recovery → paused back to false, admin changed
        address newAdmin = makeAddr("newAdmin");
        vm.prank(recovery);
        bridge.executeRecovery(newAdmin, address(0));

        (address _admin, , , , , , , , , bool _paused2, , ) = bridge.getBridgeInfo();
        assertEq(_admin, newAdmin);
        assertFalse(_paused2);
    }

    function testGetNonceStatus() public {
        // 未确认前
        (bool processed, bool confirmed) = bridge.getNonceStatus(1, relayer1);
        assertFalse(processed);
        assertFalse(confirmed);

        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        // relayer1 确认后
        vm.prank(relayer1);
        bridge.confirmEvent(data);

        (processed, confirmed) = bridge.getNonceStatus(1, relayer1);
        assertFalse(processed);
        assertTrue(confirmed);

        (processed, confirmed) = bridge.getNonceStatus(1, relayer2);
        assertFalse(processed);
        assertFalse(confirmed);

        // relayer2 确认达阈值后
        vm.prank(relayer2);
        bridge.confirmEvent(data);

        (processed, confirmed) = bridge.getNonceStatus(1, relayer1);
        assertTrue(processed);
        assertTrue(confirmed);

        (processed, confirmed) = bridge.getNonceStatus(1, relayer2);
        assertTrue(processed);
        assertTrue(confirmed);

        (processed, confirmed) = bridge.getNonceStatus(1, relayer3);
        assertTrue(processed);
        assertFalse(confirmed);

        // 非 relayer 地址
        (processed, confirmed) = bridge.getNonceStatus(1, user1);
        assertTrue(processed);
        assertFalse(confirmed);

        // 未使用的 nonce
        (processed, confirmed) = bridge.getNonceStatus(999, relayer1);
        assertFalse(processed);
        assertFalse(confirmed);
    }

    function testGetRateLimitStatus() public view {
        (
            uint64 _maxPerWindow,
            uint64 _windowDuration,
            uint64 _maxSingle,
            uint64 _maxStake,
            uint64 _minReserve,
            uint64 _windowStart,
            uint64 _windowUsage,
            uint64 _prevUsage
        ) = bridge.getRateLimitStatus();

        assertEq(_maxPerWindow, type(uint64).max);
        assertEq(_windowDuration, 3600);
        assertEq(_maxSingle, type(uint64).max);
        assertEq(_maxStake, 0);
        assertEq(_minReserve, 0);
        assertGt(_windowStart, 0);
        assertEq(_windowUsage, 0);
        assertEq(_prevUsage, 0);
    }

    function testGetRateLimitStatus_AfterUnlock() public {
        // Unlock some amount to see window usage change
        Bridge1024.StakeEventData memory data = _makeEventData(500e6, user1, 1);
        _confirmToThreshold(data);

        (, , , , , , uint64 _windowUsage, ) = bridge.getRateLimitStatus();
        assertEq(_windowUsage, 500e6);
    }

    // ========================================================================
    //                      R5 AUDIT FIXES (L-R5-1 / L-R5-2 / H-R5-2)
    // ========================================================================

    // L-R5-1: 直接以未知 calldata 调用合约（带或不带 ETH）应回退到 fallback() 并 revert
    function testFallback_RevertsOnUnknownCalldata() public {
        (bool ok, ) = address(bridge).call(hex"deadbeef");
        assertFalse(ok, "fallback() must revert on unknown calldata");

        // 携带 ETH 也应回退
        vm.deal(address(this), 1 ether);
        (bool ok2, ) = address(bridge).call{value: 1 wei}(hex"cafebabe");
        assertFalse(ok2, "payable fallback() must revert on unknown calldata + value");

        // 纯 ETH 转账依旧由 receive() 拒绝
        (bool ok3, ) = address(bridge).call{value: 1 wei}("");
        assertFalse(ok3, "receive() must revert on plain ETH transfer");
    }

    // L-R5-2: confirmEvent 在投票阶段就应拒绝超出 maxSingleUnlock 的事件，
    // 避免 relayer 投到阈值才被回滚而浪费 gas
    function testConfirmEvent_EarlyMaxSingleReject() public {
        // 收紧 maxSingle 为 100 USDC
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, 100e6, 0, 0);

        Bridge1024.StakeEventData memory data = _makeEventData(200e6, user1, 100);

        // 第一个 relayer 投票就应直接 revert（早拒），而不是攒到阈值
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.SingleTransferExceeded.selector);
        bridge.confirmEvent(data);

        // nonce 状态没有任何残留
        assertFalse(_isProcessed(100));
        (, , uint8 frozen) = bridge.nonceConfirmations(100);
        assertEq(frozen, 0, "frozenThreshold should not be set when early-rejected");
    }

    // L-R5-2: maxSingleUnlock = 0 时不强制单笔限额，等同于"未配置"
    function testConfirmEvent_EarlyMaxSingleSkippedWhenZero() public {
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, 0, 0, 0);

        Bridge1024.StakeEventData memory data = _makeEventData(50_000e6, user1, 101);

        // 不应再因 single 限额而 revert（合约金库 100k USDC，足够支付）
        vm.prank(relayer1);
        bridge.confirmEvent(data);
        vm.prank(relayer2);
        bridge.confirmEvent(data);

        assertTrue(_isProcessed(101));
    }

    // H-R5-2: 极端配置（maxPerWindow = uint64.max）下，
    // 经过 _checkRateLimit 累加多笔 unlock 不会发生 silent truncate
    function testRateLimit_U64Boundary_NoSilentTruncation() public {
        // 用 maxPerWindow = type(uint64).max、单笔无限的极端配置
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, 0, 0, 0);

        // 给桥合约铸造一大笔，避免储备不变量提前阻挡
        usdc.mint(address(bridge), uint256(type(uint64).max));

        // 第一笔接近上限的解锁应成功
        uint64 big = type(uint64).max - 1;
        Bridge1024.StakeEventData memory data1 = _makeEventData(big, user1, 200);
        _confirmToThreshold(data1);
        (, , , , , , uint64 used, ) = bridge.getRateLimitStatus();
        assertEq(used, big, "first big unlock accumulates correctly");

        // 第二笔哪怕只有 2，也应被速率限制 revert（而非溢出回到很小的数）
        Bridge1024.StakeEventData memory data2 = _makeEventData(2, user1, 201);
        vm.prank(relayer1);
        bridge.confirmEvent(data2);
        vm.prank(relayer2);
        vm.expectRevert(Bridge1024.RateLimitExceeded.selector);
        bridge.confirmEvent(data2);
    }

    // H-R5-2 fuzz：滑动窗口在任意 uint64 配置下都不会出现 silent truncate
    // 任意通过的 unlock 序列，currentWindowUsage 必然单调递增且 <= maxPerWindow
    function testFuzz_RateLimit_MonotonicAndBounded(
        uint64 maxPerWindow,
        uint64 amount1,
        uint64 amount2
    ) public {
        // 排除会立刻 revert 的 0 配置
        vm.assume(maxPerWindow > 0);
        vm.assume(amount1 > 0 && amount2 > 0);
        vm.assume(uint256(amount1) + uint256(amount2) <= maxPerWindow);

        // 单笔限额关掉，纯测速率累加；储备也关掉
        vm.prank(admin);
        bridge.configureRateLimits(maxPerWindow, 3600, 0, 0, 0);

        // 保证桥金库足够
        usdc.mint(address(bridge), uint256(amount1) + uint256(amount2));

        Bridge1024.StakeEventData memory d1 = _makeEventData(amount1, user1, 300);
        _confirmToThreshold(d1);
        (, , , , , , uint64 used1, ) = bridge.getRateLimitStatus();
        assertEq(used1, amount1, "amount1 fully recorded");

        Bridge1024.StakeEventData memory d2 = _makeEventData(amount2, user1, 301);
        _confirmToThreshold(d2);
        (, , , , , , uint64 used2, ) = bridge.getRateLimitStatus();
        assertEq(uint256(used2), uint256(amount1) + uint256(amount2), "amount2 accumulated without truncation");
        assertLe(uint256(used2), uint256(maxPerWindow));
    }
}
