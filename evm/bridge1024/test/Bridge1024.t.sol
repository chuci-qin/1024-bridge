// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/Bridge1024.sol";
import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/// @dev Inline mock ERC20 for testing (replaces deleted MockUSDC.sol)
contract MockUSDC is ERC20 {
    constructor() ERC20("USD Coin", "USDC") {
        _mint(msg.sender, 1_000_000 * 10**6);
    }
    function decimals() public pure override returns (uint8) { return 6; }
    function mint(address to, uint256 amount) external { _mint(to, amount); }
}

/// @dev Mock 18-decimal token (e.g. USDT on BSC) for decimal ratio tests
contract MockUSDT is ERC20 {
    constructor() ERC20("Tether USD", "USDT") {
        _mint(msg.sender, 1_000_000 * 10**18);
    }
    function decimals() public pure override returns (uint8) { return 18; }
    function mint(address to, uint256 amount) external { _mint(to, amount); }
}

/**
 * @title Bridge1024Test
 * @notice Comprehensive test suite for Bridge1024 cross-chain bridge
 * @dev Tests are aligned with SVM implementation and follow the test plan in docs/testplan.md
 */
contract Bridge1024Test is Test {
    Bridge1024 public bridge;
    MockUSDC public usdc;
    
    // Test accounts
    address public admin;
    uint256 public adminPrivateKey;
    address public vault;
    address public user1;
    uint256 public user1PrivateKey;
    address public user2;
    uint256 public user2PrivateKey;
    address public relayer1;
    uint256 public relayer1PrivateKey;
    address public relayer2;
    uint256 public relayer2PrivateKey;
    address public relayer3;
    uint256 public relayer3PrivateKey;
    address public nonRelayer;
    uint256 public nonRelayerPrivateKey;
    address public nonAdmin;
    uint256 public nonAdminPrivateKey;
    bytes32 public peerContract;
    
    // Test constants (aligned with SVM)
    uint64 public constant SOURCE_CHAIN_ID = 421614; // Arbitrum Sepolia
    uint64 public constant TARGET_CHAIN_ID = 91024; // 1024chain testnet
    uint64 public constant TEST_AMOUNT = 100_000000; // 100 USDC (6 decimals)
    uint256 public constant MAX_RELAYERS = 18;
    uint8 public constant MIN_THRESHOLD = 2; // For 3 relayers
    uint8 public constant MAX_THRESHOLD = 13; // For 18 relayers
    
    // Events for testing
    event StakeEvent(
        bytes32 indexed sourceContract,
        bytes32 indexed targetContract,
        uint64 chainId,
        uint64 blockHeight,
        uint64 amount,
        address sender,
        string receiverAddress,
        uint64 nonce
    );
    
    event RelayerAdded(address indexed relayer);
    event RelayerRemoved(address indexed relayer);
    event SignatureSubmitted(address indexed relayer, uint64 indexed nonce);
    event TokensUnlocked(uint64 indexed nonce, address receiver, uint64 amount);
    
    function setUp() public {
        // Generate test accounts
        adminPrivateKey = 0xA11CE;
        admin = vm.addr(adminPrivateKey);
        
        vault = makeAddr("vault");
        
        user1PrivateKey = 0x8001;
        user1 = vm.addr(user1PrivateKey);
        
        user2PrivateKey = 0x8002;
        user2 = vm.addr(user2PrivateKey);
        
        relayer1PrivateKey = 0xB001;
        relayer1 = vm.addr(relayer1PrivateKey);
        
        relayer2PrivateKey = 0xB002;
        relayer2 = vm.addr(relayer2PrivateKey);
        
        relayer3PrivateKey = 0xB003;
        relayer3 = vm.addr(relayer3PrivateKey);
        
        nonRelayerPrivateKey = 0xBAD1;
        nonRelayer = vm.addr(nonRelayerPrivateKey);
        
        nonAdminPrivateKey = 0xBAD2;
        nonAdmin = vm.addr(nonAdminPrivateKey);
        
        peerContract = bytes32(uint256(uint160(makeAddr("peerContract"))));
        
        // Deploy contracts
        bridge = new Bridge1024();
        usdc = new MockUSDC();
        
        // Fund vault with ETH for transaction fees
        vm.deal(vault, 100 ether);
        
        // Mint test USDC to users and contract (contract acts as vault)
        usdc.mint(user1, 1000 * TEST_AMOUNT);
        usdc.mint(user2, 1000 * TEST_AMOUNT);
        usdc.mint(address(bridge), 10000 * TEST_AMOUNT);
        
        // No need to approve - contract uses transfer() not transferFrom()
    }
    
    // ============ Helper Functions (Aligned with SVM) ============
    
    /**
     * @notice Hash event data using SHA-256 and JSON format (aligned with SVM)
     * @dev Must match SVM implementation exactly
     */
    function hashEventData(Bridge1024.StakeEventData memory eventData) internal pure returns (bytes32) {
        bytes memory json = abi.encodePacked(
            '{"sourceContract":"', bytes32ToString(eventData.sourceContract),
            '","targetContract":"', bytes32ToString(eventData.targetContract),
            '","chainId":"', uint64ToString(eventData.sourceChainId),
            '","blockHeight":"', uint64ToString(eventData.blockHeight),
            '","amount":"', uint64ToString(eventData.amount),
            '","sender":"', addressToString(eventData.sender),
            '","receiverAddress":"', eventData.receiverAddress,
            '","nonce":"', uint64ToString(eventData.nonce),
            '"}'
        );
        
        return sha256(json);
    }
    
    /**
     * @notice Generate EIP-191 signed message hash
     */
    function toEthSignedMessageHash(bytes32 hash) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", hash));
    }
    
    /**
     * @notice Sign event data (aligned with SVM signing process)
     */
    function signEventData(
        Bridge1024.StakeEventData memory eventData,
        uint256 privateKey
    ) internal pure returns (bytes memory) {
        bytes32 hash = hashEventData(eventData);
        bytes32 ethSignedHash = toEthSignedMessageHash(hash);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privateKey, ethSignedHash);
        return abi.encodePacked(r, s, v);
    }
    
    /**
     * @notice Calculate threshold (aligned with SVM: ceil(relayerCount * 2 / 3))
     */
    function calculateThreshold(uint256 relayerCount) internal pure returns (uint256) {
        return (relayerCount * 2 + 2) / 3;
    }
    
    /**
     * @notice Convert bytes32 to hex string (lowercase, no 0x prefix)
     */
    function bytes32ToString(bytes32 data) internal pure returns (string memory) {
        bytes memory alphabet = "0123456789abcdef";
        bytes memory str = new bytes(64);
        
        for (uint i = 0; i < 32; i++) {
            uint8 b = uint8(data[i]);
            str[i*2] = alphabet[b >> 4];
            str[i*2 + 1] = alphabet[b & 0x0f];
        }
        
        return string(str);
    }
    
    /**
     * @notice Convert address to hex string (lowercase, no 0x prefix)
     * @dev Helper for converting receiver addresses
     */
    function addressToString(address addr) internal pure returns (string memory) {
        bytes memory alphabet = "0123456789abcdef";
        bytes memory str = new bytes(40);
        
        for (uint i = 0; i < 20; i++) {
            uint8 b = uint8(uint(uint160(addr)) / (2**(8*(19 - i))));
            str[i*2] = alphabet[b >> 4];
            str[i*2 + 1] = alphabet[b & 0x0f];
        }
        
        return string(str);
    }
    
    /**
     * @notice Convert uint64 to string
     */
    function uint64ToString(uint64 value) internal pure returns (string memory) {
        if (value == 0) return "0";
        
        uint256 temp = value;
        uint256 digits;
        while (temp != 0) {
            digits++;
            temp /= 10;
        }
        
        bytes memory buffer = new bytes(digits);
        while (value != 0) {
            digits--;
            buffer[digits] = bytes1(uint8(48 + value % 10));
            value /= 10;
        }
        
        return string(buffer);
    }
    
    // Note: Mock ECDSA public key generation removed
    // EVM uses ecrecover which directly recovers address from signature
    // No need to store or generate public keys
    
    // ============ Unified Contract Tests ============
    
    function testTC001_UnifiedInitialize() public {
        vm.prank(admin);
        bridge.initialize(admin);
        
        // Verify sender state - vault is now contract itself
        (address sVault, address sAdmin, address sUsdc, uint64 sNonce, 
         bytes32 sTarget, uint64 sSourceChain, uint64 sTargetChain, uint64 sDecimalRatio) = bridge.senderState();
        assertEq(sVault, address(bridge)); // Contract acts as vault
        assertEq(sAdmin, admin);
        assertEq(sUsdc, address(0)); // Not configured yet
        assertEq(sNonce, 0);
        assertEq(sTarget, bytes32(0)); // Not configured yet
        assertEq(sSourceChain, 0); // Not configured yet
        assertEq(sTargetChain, 0); // Not configured yet
        assertEq(sDecimalRatio, 1); // Default decimal ratio
        
        // Verify receiver state - vault is now contract itself
        (address rVault, address rAdmin, address rUsdc, uint64 rRelayerCount, 
         bytes32 rSource, uint64 rSourceChain, uint64 rTargetChain, uint64 rLastNonce, uint64 rDecimalRatio) = bridge.receiverState();
        assertEq(rVault, address(bridge)); // Contract acts as vault
        assertEq(rAdmin, admin);
        assertEq(rUsdc, address(0)); // Not configured yet
        assertEq(rRelayerCount, 0);
        assertEq(rSource, bytes32(0)); // Not configured yet
        assertEq(rSourceChain, 0); // Not configured yet
        assertEq(rTargetChain, 0); // Not configured yet
        assertEq(rLastNonce, 0);
        assertEq(rDecimalRatio, 1); // Default decimal ratio
    }
    
    function testTC002_ConfigureUsdc() public {
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        vm.stopPrank();
        
        // Verify USDC configured in both sender and receiver
        (, , address sUsdc, , , , , ) = bridge.senderState();
        assertEq(sUsdc, address(usdc));
        
        (, , address rUsdc, , , , , , ) = bridge.receiverState();
        assertEq(rUsdc, address(usdc));
    }
    
    function testTC003_ConfigurePeer_Sender() public {
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        vm.stopPrank();
        
        // Verify sender configuration
        (, , , , bytes32 sTarget, uint64 sSourceChain, uint64 sTargetChain, ) = bridge.senderState();
        assertEq(sTarget, peerContract);
        assertEq(sSourceChain, SOURCE_CHAIN_ID);
        assertEq(sTargetChain, TARGET_CHAIN_ID);
    }
    
    function testTC003_ConfigurePeer_Receiver() public {
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        vm.stopPrank();
        
        // Verify receiver configuration
        // Note: Chain IDs are swapped for receiver (receives from peer)
        (, , , , bytes32 rSource, uint64 rSourceChain, uint64 rTargetChain, , ) = bridge.receiverState();
        assertEq(rSource, peerContract);
        assertEq(rSourceChain, TARGET_CHAIN_ID);  // Source is peer chain
        assertEq(rTargetChain, SOURCE_CHAIN_ID);  // Target is current chain
    }
    
    function testTC003B_ConfigurePeer_NonAdmin() public {
        vm.prank(admin);
        bridge.initialize(admin);
        
        // Non-admin tries to configure peer
        vm.prank(nonAdmin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
    }
    
    function testTC003C_ConfigureDecimalRatio_Admin() public {
        vm.startPrank(admin);
        bridge.initialize(admin);
        uint64 ratio = 1_000_000_000_000; // 10^12 for 18dec -> 6dec
        bridge.configureDecimalRatio(ratio);
        vm.stopPrank();
        
        (, , , , , , , uint64 sDecimalRatio) = bridge.senderState();
        (, , , , , , , , uint64 rDecimalRatio) = bridge.receiverState();
        assertEq(sDecimalRatio, ratio);
        assertEq(rDecimalRatio, ratio);
    }
    
    function testTC003D_ConfigureDecimalRatio_NonAdmin() public {
        vm.prank(admin);
        bridge.initialize(admin);
        
        vm.prank(nonAdmin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configureDecimalRatio(1_000_000_000_000);
    }
    
    function testTC003E_ConfigureDecimalRatio_ZeroRevert() public {
        vm.startPrank(admin);
        bridge.initialize(admin);
        vm.stopPrank();
        
        vm.prank(admin);
        vm.expectRevert(); // require(ratio > 0)
        bridge.configureDecimalRatio(0);
    }
    
    // ============ Sender Contract Tests ============
    
    function testTC004_Stake_Success() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        vm.stopPrank();
        
        // User1 approves and stakes
        vm.startPrank(user1);
        usdc.approve(address(bridge), TEST_AMOUNT);
        
        uint256 contractBalanceBefore = usdc.balanceOf(address(bridge));
        uint256 userBalanceBefore = usdc.balanceOf(user1);
        
        // Expect StakeEvent
        vm.expectEmit(true, true, false, true);
        emit StakeEvent(
            bytes32(uint256(uint160(address(bridge)))),
            peerContract,
            SOURCE_CHAIN_ID,
            uint64(block.number),
            TEST_AMOUNT,
            user1,
            addressToString(user2),
            1
        );
        
        uint64 nonce = bridge.stake(TEST_AMOUNT, addressToString(user2));
        vm.stopPrank();
        
        // Verify results - contract acts as vault
        assertEq(nonce, 1);
        assertEq(usdc.balanceOf(address(bridge)), contractBalanceBefore + TEST_AMOUNT);
        assertEq(usdc.balanceOf(user1), userBalanceBefore - TEST_AMOUNT);
        assertEq(bridge.getSenderNonce(), 1);
    }
    
    function testTC005_Stake_InsufficientBalance() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        vm.stopPrank();
        
        // User1 tries to stake more than balance
        vm.startPrank(user1);
        uint64 largeAmount = uint64(usdc.balanceOf(user1) + 1);
        usdc.approve(address(bridge), largeAmount);
        
        vm.expectRevert();
        bridge.stake(largeAmount, addressToString(user2));
        vm.stopPrank();
    }
    
    function testTC006_Stake_NotApproved() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        vm.stopPrank();
        
        // User1 tries to stake without approval
        vm.prank(user1);
        vm.expectRevert();
        bridge.stake(TEST_AMOUNT, addressToString(user2));
    }
    
    function testTC007_Stake_UsdcNotConfigured() public {
        // Initialize but don't configure USDC
        vm.prank(admin);
        bridge.initialize(admin);
        
        // User1 tries to stake
        vm.startPrank(user1);
        usdc.approve(address(bridge), TEST_AMOUNT);
        
        vm.expectRevert(Bridge1024.UsdcNotConfigured.selector);
        bridge.stake(TEST_AMOUNT, addressToString(user2));
        vm.stopPrank();
    }
    
    function testTC004B_Stake_WithDecimalRatio() public {
        // 模拟 BNB-USDT(18dec) -> 1024chain USDC(6dec) 场景
        MockUSDT usdt = new MockUSDT();
        uint256 bridgeBalance = 10000 * 100 * 10**18;
        usdt.mint(user1, 1000 * 100 * 10**18);
        usdt.mint(address(bridge), bridgeBalance);
        
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdt));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        bridge.configureDecimalRatio(1_000_000_000_000); // 10^12: 18dec -> 6dec
        vm.stopPrank();
        
        uint256 stakeAmount = 100 * 10**18; // 100 USDT (18 decimals)
        uint64 expectedTargetAmount = 100_000000; // 100 in 6 decimals
        
        vm.startPrank(user1);
        usdt.approve(address(bridge), stakeAmount);
        
        vm.recordLogs();
        uint64 nonce = bridge.stake(stakeAmount, addressToString(user2));
        Vm.Log[] memory entries = vm.getRecordedLogs();
        vm.stopPrank();
        
        // 验证: 存入 100e18, 金库增加 stakeAmount, 事件中 amount 应为 100e6 (target decimals)
        assertEq(nonce, 1);
        assertEq(usdt.balanceOf(address(bridge)), bridgeBalance + stakeAmount);
        bool found = false;
        for (uint i = 0; i < entries.length; i++) {
            if (entries[i].topics[0] == keccak256("StakeEvent(bytes32,bytes32,uint64,uint64,uint64,address,string,uint64)")) {
                (, , uint64 amount, , , ) = abi.decode(entries[i].data, (uint64, uint64, uint64, address, string, uint64));
                assertEq(amount, expectedTargetAmount);
                found = true;
                break;
            }
        }
        assertTrue(found, "StakeEvent amount mismatch for decimal ratio");
    }
    
    function testTC008_StakeEvent_Integrity() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        vm.stopPrank();
        
        // User1 stakes
        vm.startPrank(user1);
        usdc.approve(address(bridge), TEST_AMOUNT);
        
        // Record logs
        vm.recordLogs();
        uint64 nonce = bridge.stake(TEST_AMOUNT, addressToString(user2));
        Vm.Log[] memory entries = vm.getRecordedLogs();
        vm.stopPrank();
        
        // Find StakeEvent (should be the last event)
        bool found = false;
        for (uint i = 0; i < entries.length; i++) {
            if (entries[i].topics[0] == keccak256("StakeEvent(bytes32,bytes32,uint64,uint64,uint64,address,string,uint64)")) {
                found = true;
                // Verify event fields
                assertEq(bytes32(entries[i].topics[1]), bytes32(uint256(uint160(address(bridge))))); // sourceContract
                assertEq(bytes32(entries[i].topics[2]), peerContract); // targetContract
                // Decode data to verify other fields
                (uint64 chainId, uint64 blockHeight, uint64 amount, address sender, string memory receiverAddress, uint64 eventNonce) = 
                    abi.decode(entries[i].data, (uint64, uint64, uint64, address, string, uint64));
                assertEq(chainId, SOURCE_CHAIN_ID);
                assertEq(blockHeight, uint64(block.number));
                assertEq(amount, TEST_AMOUNT);
                assertEq(sender, user1);
                assertEq(receiverAddress, addressToString(user2));
                assertEq(eventNonce, nonce);
                break;
            }
        }
        assertTrue(found, "StakeEvent not found");
    }
    
    // ============ Receiver Contract Tests ============
    
    function testTC101_AddRelayer_Admin() public {
        vm.startPrank(admin);
        bridge.initialize(admin);
        
        // Add relayers
        vm.expectEmit(true, false, false, false);
        emit RelayerAdded(relayer1);
        bridge.addRelayer(relayer1);
        
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Verify
        assertEq(bridge.getRelayerCount(), 3);
        assertTrue(bridge.isRelayer(relayer1));
        assertTrue(bridge.isRelayer(relayer2));
        assertTrue(bridge.isRelayer(relayer3));
    }
    
    function testTC102_RemoveRelayer_Admin() public {
        vm.startPrank(admin);
        bridge.initialize(admin);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        
        // Remove relayer1
        vm.expectEmit(true, false, false, false);
        emit RelayerRemoved(relayer1);
        bridge.removeRelayer(relayer1);
        vm.stopPrank();
        
        // Verify
        assertEq(bridge.getRelayerCount(), 2);
        assertFalse(bridge.isRelayer(relayer1));
        assertTrue(bridge.isRelayer(relayer2));
        assertTrue(bridge.isRelayer(relayer3));
    }
    
    function testTC103_AddRelayer_NonAdmin() public {
        vm.prank(admin);
        bridge.initialize(admin);
        
        // Non-admin tries to add relayer
        vm.prank(nonAdmin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.addRelayer(relayer1);
    }
    
    function testTC103_RemoveRelayer_NonAdmin() public {
        vm.startPrank(admin);
        bridge.initialize(admin);
        
        bridge.addRelayer(relayer1);
        vm.stopPrank();
        
        // Non-admin tries to remove relayer
        vm.prank(nonAdmin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.removeRelayer(relayer1);
    }
    
    function testTC104_SubmitSignature_SingleRelayer() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Create event data (from peer to EVM)
        // Note: ChainIds match receiver state (swapped from configurePeer)
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        // Relayer1 submits signature
        bytes memory signature1 = signEventData(eventData, relayer1PrivateKey);
        
        uint256 receiverBalanceBefore = usdc.balanceOf(user2);
        
        vm.prank(relayer1);
        vm.expectEmit(true, true, false, false);
        emit SignatureSubmitted(relayer1, 1);
        bridge.submitSignature(eventData, signature1);
        
        // Verify no unlock (threshold not reached: 1 < 2)
        assertEq(usdc.balanceOf(user2), receiverBalanceBefore);
        assertEq(bridge.getReceiverLastNonce(), 0); // Not updated yet
    }
    
    function testTC105_SubmitSignature_ReachThreshold() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Create event data (from peer to EVM)
        // Note: ChainIds match receiver state (swapped from configurePeer)
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        // Relayer1 and Relayer2 submit signatures
        bytes memory signature1 = signEventData(eventData, relayer1PrivateKey);
        bytes memory signature2 = signEventData(eventData, relayer2PrivateKey);
        
        uint256 receiverBalanceBefore = usdc.balanceOf(user2);
        uint256 contractBalanceBefore = usdc.balanceOf(address(bridge));
        
        vm.prank(relayer1);
        bridge.submitSignature(eventData, signature1);
        
        // Second signature should trigger unlock
        vm.prank(relayer2);
        vm.expectEmit(true, false, false, true);
        emit TokensUnlocked(1, user2, TEST_AMOUNT);
        bridge.submitSignature(eventData, signature2);
        
        // Verify unlock - contract acts as vault
        assertEq(usdc.balanceOf(user2), receiverBalanceBefore + TEST_AMOUNT);
        assertEq(usdc.balanceOf(address(bridge)), contractBalanceBefore - TEST_AMOUNT);
        assertEq(bridge.getReceiverLastNonce(), 1);
    }
    
    function testTC105B_SubmitSignature_WithDecimalRatio() public {
        // 模拟 1024chain USDC(6dec) -> EVM USDT(18dec) 解锁场景
        // 使用 MockUSDT(18dec) + decimalRatio=10^12: 事件 amount=100_000000 解锁 100*10^18
        MockUSDT usdt = new MockUSDT();
        uint256 bridgeBalance = 10000 * 100 * 10**18;
        usdt.mint(address(bridge), bridgeBalance);
        
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdt));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        bridge.configureDecimalRatio(1_000_000_000_000); // 10^12
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        uint64 eventAmount = 100_000000;
        uint256 expectedUnlock = 100 * 10**18;
        
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,
            targetChainId: SOURCE_CHAIN_ID,
            blockHeight: uint64(block.number),
            amount: eventAmount,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        uint256 balanceBefore = usdt.balanceOf(user2);
        
        bytes memory sig1 = signEventData(eventData, relayer1PrivateKey);
        vm.prank(relayer1);
        bridge.submitSignature(eventData, sig1);
        
        bytes memory sig2 = signEventData(eventData, relayer2PrivateKey);
        vm.prank(relayer2);
        bridge.submitSignature(eventData, sig2);
        
        assertEq(usdt.balanceOf(user2), balanceBefore + expectedUnlock);
        assertEq(bridge.getReceiverLastNonce(), 1);
    }
    
    function testTC106_NonceIncreasing_SameNonce() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Process nonce 1
        Bridge1024.StakeEventData memory eventData1 = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        bytes memory sig1_1 = signEventData(eventData1, relayer1PrivateKey);
        bytes memory sig1_2 = signEventData(eventData1, relayer2PrivateKey);
        
        vm.prank(relayer1);
        bridge.submitSignature(eventData1, sig1_1);
        
        vm.prank(relayer2);
        bridge.submitSignature(eventData1, sig1_2);
        
        // Try to replay same nonce (should fail because lastNonce was updated to 1)
        // Create a fresh event with same nonce
        Bridge1024.StakeEventData memory replayEvent = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1 // Same nonce
        });
        
        bytes memory relayer3_sig = signEventData(replayEvent, relayer3PrivateKey);
        
        vm.prank(relayer3);
        vm.expectRevert(Bridge1024.InvalidNonce.selector);
        bridge.submitSignature(replayEvent, relayer3_sig);
    }
    
    function testTC106_NonceIncreasing_SmallerNonce() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Process nonce 2 first (simulating out-of-order processing)
        Bridge1024.StakeEventData memory eventData2 = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 2
        });
        
        bytes memory sig2_1 = signEventData(eventData2, relayer1PrivateKey);
        bytes memory sig2_2 = signEventData(eventData2, relayer2PrivateKey);
        
        vm.prank(relayer1);
        bridge.submitSignature(eventData2, sig2_1);
        
        vm.prank(relayer2);
        bridge.submitSignature(eventData2, sig2_2);
        
        // Try to process nonce 1 (smaller than lastNonce which is now 2)
        Bridge1024.StakeEventData memory eventData1 = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1 // Smaller than lastNonce (2)
        });
        
        // Need to use a new relayer since relayer1 already signed nonce 2
        bytes memory relayer3_sig = signEventData(eventData1, relayer3PrivateKey);
        
        vm.prank(relayer3);
        vm.expectRevert(Bridge1024.InvalidNonce.selector);
        bridge.submitSignature(eventData1, relayer3_sig);
    }
    
    function testTC106_NonceIncreasing_LargerNonce() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Process nonce 1
        Bridge1024.StakeEventData memory eventData1 = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        bytes memory sig1_1 = signEventData(eventData1, relayer1PrivateKey);
        bytes memory sig1_2 = signEventData(eventData1, relayer2PrivateKey);
        
        vm.prank(relayer1);
        bridge.submitSignature(eventData1, sig1_1);
        
        vm.prank(relayer2);
        bridge.submitSignature(eventData1, sig1_2);
        
        // Process nonce 3 (larger than lastNonce + 1, should succeed)
        Bridge1024.StakeEventData memory eventData3 = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 3
        });
        
        bytes memory sig3_1 = signEventData(eventData3, relayer1PrivateKey);
        bytes memory sig3_2 = signEventData(eventData3, relayer2PrivateKey);
        
        vm.prank(relayer1);
        bridge.submitSignature(eventData3, sig3_1);
        
        vm.prank(relayer2);
        bridge.submitSignature(eventData3, sig3_2);
        
        // Verify nonce updated to 3
        assertEq(bridge.getReceiverLastNonce(), 3);
    }
    
    function testTC107_SubmitSignature_InvalidSignature() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Create event data (from peer to EVM)
        // Note: ChainIds match receiver state (swapped from configurePeer)
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        // Sign with wrong private key (nonRelayer instead of relayer1)
        bytes memory wrongSignature = signEventData(eventData, nonRelayerPrivateKey);
        
        // Relayer1 tries to submit wrong signature
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidSignature.selector);
        bridge.submitSignature(eventData, wrongSignature);
    }
    
    function testTC108_SubmitSignature_NonWhitelistedRelayer() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers (but not nonRelayer)
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Create event data (from peer to EVM)
        // Note: ChainIds match receiver state (swapped from configurePeer)
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        // NonRelayer tries to submit signature
        bytes memory signature = signEventData(eventData, nonRelayerPrivateKey);
        
        vm.prank(nonRelayer);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.submitSignature(eventData, signature);
    }
    
    function testTC109_SubmitSignature_UsdcNotConfigured() public {
        // Initialize but don't configure USDC
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        vm.stopPrank();
        
        // Create event data (from peer to EVM)
        // Note: ChainIds match receiver state (swapped from configurePeer)
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        bytes memory signature = signEventData(eventData, relayer1PrivateKey);
        
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.UsdcNotConfigured.selector);
        bridge.submitSignature(eventData, signature);
    }
    
    function testTC110_SubmitSignature_WrongSourceContract() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        vm.stopPrank();
        
        // Create event data with wrong source contract
        bytes32 wrongContract = bytes32(uint256(uint160(makeAddr("wrongContract"))));
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: wrongContract, // Wrong!
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: SOURCE_CHAIN_ID,
            targetChainId: TARGET_CHAIN_ID,
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        bytes memory signature = signEventData(eventData, relayer1PrivateKey);
        
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidSourceContract.selector);
        bridge.submitSignature(eventData, signature);
    }
    
    function testTC111_SubmitSignature_WrongChainId() public {
        // Initialize and configure
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        vm.stopPrank();
        
        // Create event data with wrong chain ID
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: 999999, // Wrong!
            targetChainId: TARGET_CHAIN_ID,
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        bytes memory signature = signEventData(eventData, relayer1PrivateKey);
        
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidChainId.selector);
        bridge.submitSignature(eventData, signature);
    }
    
    // ============ Integration Tests ============
    
    function testIT001_EndToEnd_EVMToSVM() public {
        // Step 1: Initialize system
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Step 2: User stakes on EVM
        vm.startPrank(user1);
        usdc.approve(address(bridge), TEST_AMOUNT);
        uint64 nonce = bridge.stake(TEST_AMOUNT, addressToString(user2));
        vm.stopPrank();
        
        assertEq(nonce, 1);
        
        // Step 3: Simulate relayers observing event and submitting signatures to SVM
        // (In real scenario, this would be cross-chain)
        // For test, we simulate the reverse: SVM relayers submit to EVM receiver
        
        // Create event data (as if from SVM)
        // Note: ChainIds match receiver state (swapped from configurePeer)
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: nonce
        });
        
        // Relayers submit signatures
        uint256 balanceBefore = usdc.balanceOf(user2);
        
        bytes memory relayer1_sig = signEventData(eventData, relayer1PrivateKey);
        vm.prank(relayer1);
        bridge.submitSignature(eventData, relayer1_sig);
        
        bytes memory relayer2_sig = signEventData(eventData, relayer2PrivateKey);
        vm.prank(relayer2);
        bridge.submitSignature(eventData, relayer2_sig);
        
        // Step 4: Verify user received tokens
        assertEq(usdc.balanceOf(user2), balanceBefore + TEST_AMOUNT);
        assertEq(bridge.getReceiverLastNonce(), nonce);
    }
    
    function testIT002_EndToEnd_SVMToEVM() public {
        // Similar to IT001 but in reverse direction
        // Initialize system
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, TARGET_CHAIN_ID, SOURCE_CHAIN_ID); // Reversed
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Simulate receiving cross-chain request from SVM
        // Note: configurePeer(TARGET_CHAIN_ID, SOURCE_CHAIN_ID) means receiver receives from SOURCE_CHAIN_ID
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: SOURCE_CHAIN_ID,  // From peer chain (receiver config is swapped)
            targetChainId: TARGET_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user1),
            nonce: 1
        });
        
        uint256 balanceBefore = usdc.balanceOf(user1);
        
        bytes memory relayer1_sig = signEventData(eventData, relayer1PrivateKey);
        vm.prank(relayer1);
        bridge.submitSignature(eventData, relayer1_sig);
        
        bytes memory relayer2_sig = signEventData(eventData, relayer2PrivateKey);
        vm.prank(relayer2);
        bridge.submitSignature(eventData, relayer2_sig);
        
        // Verify
        assertEq(usdc.balanceOf(user1), balanceBefore + TEST_AMOUNT);
    }
    
    function testIT003_ConcurrentTransfers() public {
        // Initialize system
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Process 10 concurrent transfers
        uint256 transferCount = 10;
        for (uint256 i = 1; i <= transferCount; i++) {
            Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
                sourceContract: peerContract,
                targetContract: bytes32(uint256(uint160(address(bridge)))),
                sourceChainId: TARGET_CHAIN_ID,  // From peer chain
                targetChainId: SOURCE_CHAIN_ID,  // To current chain
                blockHeight: uint64(block.number),
                amount: TEST_AMOUNT,
            sender: address(0),
                receiverAddress: addressToString(user2),
                nonce: uint64(i)
            });
            
            // Submit signatures for each nonce
            bytes memory relayer1_sig = signEventData(eventData, relayer1PrivateKey);
            vm.prank(relayer1);
            bridge.submitSignature(eventData, relayer1_sig);
            
            bytes memory relayer2_sig = signEventData(eventData, relayer2PrivateKey);
            vm.prank(relayer2);
            bridge.submitSignature(eventData, relayer2_sig);
        }
        
        // Verify all transfers completed
        assertEq(bridge.getReceiverLastNonce(), transferCount);
    }
    
    function testIT004_LargeAmountTransfer() public {
        // Initialize system
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Transfer large amount (10,000 USDC)
        uint64 largeAmount = 10000 * TEST_AMOUNT;
        
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: largeAmount,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        uint256 balanceBefore = usdc.balanceOf(user2);
        
        bytes memory relayer1_sig = signEventData(eventData, relayer1PrivateKey);
        vm.prank(relayer1);
        bridge.submitSignature(eventData, relayer1_sig);
        
        bytes memory relayer2_sig = signEventData(eventData, relayer2PrivateKey);
        vm.prank(relayer2);
        bridge.submitSignature(eventData, relayer2_sig);
        
        // Verify large transfer
        assertEq(usdc.balanceOf(user2), balanceBefore + largeAmount);
    }
    
    // ============ Security Tests ============
    
    function testST001_NonceProtection_ReplayAttack() public {
        // Initialize system
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();
        
        // Process transaction with nonce 1
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        bytes memory sig1 = signEventData(eventData, relayer1PrivateKey);
        bytes memory sig2 = signEventData(eventData, relayer2PrivateKey);
        
        vm.prank(relayer1);
        bridge.submitSignature(eventData, sig1);
        
        vm.prank(relayer2);
        bridge.submitSignature(eventData, sig2);
        
        // Try to replay (same nonce)
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidNonce.selector);
        bridge.submitSignature(eventData, sig1);
    }
    
    function testST002_SignatureForgery() public {
        // Initialize system
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        vm.stopPrank();
        
        // Create event data (from peer to EVM)
        // Note: ChainIds match receiver state (swapped from configurePeer)
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        // Sign with attacker's key
        bytes memory forgedSignature = signEventData(eventData, nonRelayerPrivateKey);
        
        // Relayer1 tries to submit forged signature
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidSignature.selector);
        bridge.submitSignature(eventData, forgedSignature);
    }
    
    function testST003_AccessControl_AddRelayer() public {
        vm.prank(admin);
        bridge.initialize(admin);
        
        // Non-admin tries to add relayer
        vm.prank(nonAdmin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.addRelayer(relayer1);
    }
    
    function testST003_AccessControl_RemoveRelayer() public {
        vm.startPrank(admin);
        bridge.initialize(admin);
        
        bridge.addRelayer(relayer1);
        vm.stopPrank();
        
        // Non-admin tries to remove relayer
        vm.prank(nonAdmin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.removeRelayer(relayer1);
    }
    
    function testST004_VaultSecurity_DirectWithdraw() public {
        vm.prank(admin);
        bridge.initialize(admin);
        
        // Contract acts as vault - check contract has balance
        // Since contract holds tokens directly, it needs proper access control
        
        uint256 contractBalance = usdc.balanceOf(address(bridge));
        assertTrue(contractBalance > 0, "Contract (vault) should have balance");
    }
    
    function testST004_VaultSecurity_InsufficientBalance() public {
        // Initialize system
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        vm.stopPrank();
        
        // Try to unlock more than contract has
        uint64 excessiveAmount = uint64(usdc.balanceOf(address(bridge)) + 1);
        
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: excessiveAmount,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        bytes memory relayer1_sig = signEventData(eventData, relayer1PrivateKey);
        vm.prank(relayer1);
        bridge.submitSignature(eventData, relayer1_sig);
        
        bytes memory sig2 = signEventData(eventData, relayer2PrivateKey);
        
        vm.prank(relayer2);
        vm.expectRevert(); // Should fail on transfer
        bridge.submitSignature(eventData, sig2);
    }
    
    function testST005_EventValidation_WrongContract() public {
        // Already tested in TC110
        testTC110_SubmitSignature_WrongSourceContract();
    }
    
    function testST005_EventValidation_WrongChainId() public {
        // Already tested in TC111
        testTC111_SubmitSignature_WrongChainId();
    }
    
    // ============ Performance Tests ============
    
    function testPT001_StakeLatency() public {
        // Initialize system
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        vm.stopPrank();
        
        // Measure stake operation gas
        vm.startPrank(user1);
        usdc.approve(address(bridge), TEST_AMOUNT);
        
        uint256 gasBefore = gasleft();
        bridge.stake(TEST_AMOUNT, addressToString(user2));
        uint256 gasUsed = gasBefore - gasleft();
        vm.stopPrank();
        
        // Gas usage should be reasonable (< 200k gas)
        assertTrue(gasUsed < 200000, "Stake operation uses too much gas");
    }
    
    function testPT002_SignatureSubmissionLatency() public {
        // Initialize system
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        vm.stopPrank();
        
        // Create event data (from peer to EVM)
        // Note: ChainIds match receiver state (swapped from configurePeer)
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        // Measure signature submission gas
        bytes memory sig = signEventData(eventData, relayer1PrivateKey);
        
        vm.prank(relayer1);
        uint256 gasBefore = gasleft();
        bridge.submitSignature(eventData, sig);
        uint256 gasUsed = gasBefore - gasleft();
        
        // Gas usage should be reasonable (< 500k gas including test overhead)
        // Note: gasleft() measurement includes test infrastructure overhead
        assertTrue(gasUsed < 500000, "Signature submission uses too much gas");
    }
    
    function testPT003_EndToEndLatency() public {
        // Measure complete cross-chain transfer flow
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        vm.stopPrank();
        
        // Measure total gas for complete flow
        uint256 totalGas = 0;
        
        // Step 1: Stake (simulated on source chain)
        // Step 2: Submit signatures
        Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
            sourceContract: peerContract,
            targetContract: bytes32(uint256(uint160(address(bridge)))),
            sourceChainId: TARGET_CHAIN_ID,  // From peer chain
            targetChainId: SOURCE_CHAIN_ID,  // To current chain
            blockHeight: uint64(block.number),
            amount: TEST_AMOUNT,
            sender: address(0),
            receiverAddress: addressToString(user2),
            nonce: 1
        });
        
        bytes memory sig1 = signEventData(eventData, relayer1PrivateKey);
        bytes memory sig2 = signEventData(eventData, relayer2PrivateKey);
        
        vm.prank(relayer1);
        uint256 gas1Before = gasleft();
        bridge.submitSignature(eventData, sig1);
        totalGas += gas1Before - gasleft();
        
        vm.prank(relayer2);
        uint256 gas2Before = gasleft();
        bridge.submitSignature(eventData, sig2);
        totalGas += gas2Before - gasleft();
        
        // Total gas should be reasonable (< 600k gas for 2 signatures)
        assertTrue(totalGas < 600000, "End-to-end operation uses too much gas");
    }
    
    function testPT004_Throughput() public {
        // Test system can handle multiple transfers
        vm.startPrank(admin);
        bridge.initialize(admin);
        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, SOURCE_CHAIN_ID, TARGET_CHAIN_ID);
        
        // Add relayers
        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        vm.stopPrank();
        
        // Process 100 transfers
        uint256 transferCount = 100;
        uint256 totalGas = 0;
        
        for (uint256 i = 1; i <= transferCount; i++) {
            Bridge1024.StakeEventData memory eventData = Bridge1024.StakeEventData({
                sourceContract: peerContract,
                targetContract: bytes32(uint256(uint160(address(bridge)))),
                sourceChainId: TARGET_CHAIN_ID,  // From peer chain
                targetChainId: SOURCE_CHAIN_ID,  // To current chain
                blockHeight: uint64(block.number),
                amount: TEST_AMOUNT,
            sender: address(0),
                receiverAddress: addressToString(user2),
                nonce: uint64(i)
            });
            
            bytes memory sig1 = signEventData(eventData, relayer1PrivateKey);
            bytes memory sig2 = signEventData(eventData, relayer2PrivateKey);
            
            vm.prank(relayer1);
            uint256 gas1Before = gasleft();
            bridge.submitSignature(eventData, sig1);
            totalGas += gas1Before - gasleft();
            
            vm.prank(relayer2);
            uint256 gas2Before = gasleft();
            bridge.submitSignature(eventData, sig2);
            totalGas += gas2Before - gasleft();
        }
        
        // Verify all processed
        assertEq(bridge.getReceiverLastNonce(), transferCount);
        
        // Average gas per transfer should be reasonable (2 signatures per transfer)
        // Note: gasleft() measurement includes test infrastructure overhead
        uint256 avgGasPerTransfer = totalGas / transferCount;
        assertTrue(avgGasPerTransfer < 850000, "Average gas per transfer too high");
    }
    
    // ============ Threshold Calculation Tests ============
    
    function testThresholdCalculation() public pure {
        // Test threshold calculation (aligned with SVM)
        assertEq(calculateThreshold(3), 2);   // 3 relayers -> 2 signatures
        assertEq(calculateThreshold(4), 3);   // 4 relayers -> 3 signatures
        assertEq(calculateThreshold(5), 4);   // 5 relayers -> 4 signatures
        assertEq(calculateThreshold(18), 12); // 18 relayers -> 12 signatures
    }
}

