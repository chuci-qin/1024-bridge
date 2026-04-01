// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/Bridge1024.sol";
import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

// ============ Mock Tokens ============

contract MockUSDC is ERC20 {
    constructor() ERC20("USD Coin", "USDC") {}
    function decimals() public pure override returns (uint8) { return 6; }
    function mint(address to, uint256 amount) external { _mint(to, amount); }
}

contract MockUSDT is ERC20 {
    constructor() ERC20("Tether USD", "USDT") {}
    function decimals() public pure override returns (uint8) { return 18; }
    function mint(address to, uint256 amount) external { _mint(to, amount); }
}

// ============ Test Contract ============

contract Bridge1024Test is Test {
    Bridge1024 public bridge;
    MockUSDC public usdc;
    MockUSDT public usdt;

    address public admin;
    uint256 public adminKey;

    address public user1;
    address public user2;

    address public relayer1;
    uint256 public relayer1Key;
    address public relayer2;
    uint256 public relayer2Key;
    address public relayer3;
    uint256 public relayer3Key;

    bytes32 public peerContract = bytes32(uint256(0xdeadbeefcafe));
    uint64 public sourceChainId = 1;
    uint64 public targetChainId = 2;

    uint256 constant _SECP256K1_ORDER =
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    uint256 constant _SECP256K1_HALF_ORDER =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    // Re-declare events for vm.expectEmit
    event StakeEvent(
        bytes32 indexed sourceContract,
        bytes32 indexed targetContract,
        uint64 chainId,
        uint64 blockHeight,
        uint64 amount,
        bytes32 sender,
        string receiverAddress,
        uint64 nonce
    );
    event RelayerAdded(address indexed relayer);
    event RelayerRemoved(address indexed relayer);
    event SignatureSubmitted(address indexed relayer, uint64 indexed nonce);
    event TokensUnlocked(uint64 indexed nonce, address receiver, uint64 amount);
    event AdminTransferProposed(address indexed currentAdmin, address indexed pendingAdmin);
    event AdminTransferAccepted(address indexed oldAdmin, address indexed newAdmin);

    // ============ setUp ============

    function setUp() public {
        (admin, adminKey) = makeAddrAndKey("admin");
        user1 = makeAddr("user1");
        user2 = makeAddr("user2");
        (relayer1, relayer1Key) = makeAddrAndKey("relayer1");
        (relayer2, relayer2Key) = makeAddrAndKey("relayer2");
        (relayer3, relayer3Key) = makeAddrAndKey("relayer3");

        vm.startPrank(admin);
        bridge = new Bridge1024(admin);
        usdc = new MockUSDC();
        usdt = new MockUSDT();

        bridge.configureUsdc(address(usdc));
        bridge.configurePeer(peerContract, sourceChainId, targetChainId);
        bridge.configureRateLimits(type(uint64).max, 3600, type(uint64).max, 0);

        bridge.addRelayer(relayer1);
        bridge.addRelayer(relayer2);
        bridge.addRelayer(relayer3);
        vm.stopPrank();

        usdc.mint(user1, 10_000e6);
        usdc.mint(user2, 10_000e6);
        usdc.mint(address(bridge), 100_000e6);

        usdt.mint(user1, 10_000e18);
        usdt.mint(address(bridge), 100_000e18);

        vm.prank(user1);
        usdc.approve(address(bridge), type(uint256).max);
        vm.prank(user2);
        usdc.approve(address(bridge), type(uint256).max);
        vm.prank(user1);
        usdt.approve(address(bridge), type(uint256).max);
    }

    // ============ Helpers ============

    function _bytes32ToHex(bytes32 data) internal pure returns (string memory) {
        bytes memory alphabet = "0123456789abcdef";
        bytes memory str = new bytes(64);
        for (uint256 i = 0; i < 32; i++) {
            uint8 b = uint8(data[i]);
            str[i * 2] = alphabet[b >> 4];
            str[i * 2 + 1] = alphabet[b & 0x0f];
        }
        return string(str);
    }

    function _uint64ToString(uint64 value) internal pure returns (string memory) {
        if (value == 0) return "0";
        uint256 temp = value;
        uint256 digits;
        while (temp != 0) { digits++; temp /= 10; }
        bytes memory buffer = new bytes(digits);
        while (value != 0) {
            digits--;
            buffer[digits] = bytes1(uint8(48 + value % 10));
            value /= 10;
        }
        return string(buffer);
    }

    function _addressToHex(address addr) internal pure returns (string memory) {
        bytes memory alphabet = "0123456789abcdef";
        bytes memory str = new bytes(40);
        uint160 value = uint160(addr);
        for (uint256 i = 0; i < 20; i++) {
            uint8 b = uint8(value >> (8 * (19 - i)));
            str[i * 2] = alphabet[b >> 4];
            str[i * 2 + 1] = alphabet[b & 0x0f];
        }
        return string(str);
    }

    function _hashEventData(Bridge1024.StakeEventData memory d) internal pure returns (bytes32) {
        bytes memory part1 = abi.encodePacked(
            '{"sourceContract":"', _bytes32ToHex(d.sourceContract),
            '","targetContract":"', _bytes32ToHex(d.targetContract),
            '","chainId":"', _uint64ToString(d.sourceChainId),
            '","blockHeight":"', _uint64ToString(d.blockHeight)
        );
        bytes memory part2 = abi.encodePacked(
            '","amount":"', _uint64ToString(d.amount),
            '","sender":"', _bytes32ToHex(d.sender),
            '","receiverAddress":"', d.receiverAddress,
            '","nonce":"', _uint64ToString(d.nonce),
            '"}'
        );
        return sha256(abi.encodePacked(part1, part2));
    }

    function _signEventData(
        Bridge1024.StakeEventData memory data,
        uint256 privateKey
    ) internal view returns (bytes memory) {
        bytes32 hash = _hashEventData(data);
        bytes32 ethSignedHash = keccak256(
            abi.encodePacked("\x19Ethereum Signed Message:\n32", hash)
        );
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privateKey, ethSignedHash);
        if (uint256(s) > _SECP256K1_HALF_ORDER) {
            s = bytes32(_SECP256K1_ORDER - uint256(s));
            v = (v == 27) ? 28 : 27;
        }
        return abi.encodePacked(r, s, v);
    }

    function _signEventDataHighS(
        Bridge1024.StakeEventData memory data,
        uint256 privateKey
    ) internal view returns (bytes memory) {
        bytes32 hash = _hashEventData(data);
        bytes32 ethSignedHash = keccak256(
            abi.encodePacked("\x19Ethereum Signed Message:\n32", hash)
        );
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privateKey, ethSignedHash);
        if (uint256(s) <= _SECP256K1_HALF_ORDER) {
            s = bytes32(_SECP256K1_ORDER - uint256(s));
            v = (v == 27) ? 28 : 27;
        }
        return abi.encodePacked(r, s, v);
    }

    function _calculateThreshold(uint256 count) internal pure returns (uint8) {
        return uint8((count * 2 + 2) / 3);
    }

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
            receiverAddress: _addressToHex(receiver),
            nonce: nonce
        });
    }

    function _submitSignaturesToThreshold(Bridge1024.StakeEventData memory data) internal {
        uint8 threshold = _calculateThreshold(bridge.getRelayerCount());
        uint256[3] memory keys = [relayer1Key, relayer2Key, relayer3Key];
        address[3] memory addrs = [relayer1, relayer2, relayer3];
        for (uint8 i = 0; i < threshold; i++) {
            bytes memory sig = _signEventData(data, keys[i]);
            vm.prank(addrs[i]);
            bridge.submitSignature(data, sig);
        }
    }

    // ========================================================================
    //                         INITIALIZATION TESTS
    // ========================================================================

    function testInitialize() public view {
        (address vault, address adm,,,,,,, uint64 decRatio) = bridge.senderState();
        assertEq(vault, address(bridge));
        assertEq(adm, admin);
        assertEq(decRatio, 1);

        (address rVault, address rAdm,,,,,,,) = bridge.receiverState();
        assertEq(rVault, address(bridge));
        assertEq(rAdm, admin);
    }

    function testConfigureUsdc() public {
        MockUSDC newUsdc = new MockUSDC();
        vm.prank(admin);
        bridge.configureUsdc(address(newUsdc));

        (,, address usdcAddr,,,,,,) = bridge.senderState();
        assertEq(usdcAddr, address(newUsdc));

        vm.prank(admin);
        vm.expectRevert(Bridge1024.ZeroAddress.selector);
        bridge.configureUsdc(address(0));
    }

    function testConfigurePeer() public {
        bytes32 newPeer = bytes32(uint256(0xabcdef));
        vm.prank(admin);
        bridge.configurePeer(newPeer, 10, 20);

        (,,,, bytes32 sTgt, uint64 sSrc, uint64 sTgtCid,,) = bridge.senderState();
        assertEq(sTgt, newPeer);
        assertEq(sSrc, 10);
        assertEq(sTgtCid, 20);

        (,,,, bytes32 rSrc, uint64 rSrc2, uint64 rTgt,,) = bridge.receiverState();
        assertEq(rSrc, newPeer);
        assertEq(rSrc2, 20);
        assertEq(rTgt, 10);
    }

    function testConfigureDecimalRatio() public {
        vm.prank(admin);
        bridge.configureDecimalRatio(1e12);
        (,,,,,,,, uint64 ratio) = bridge.senderState();
        assertEq(ratio, 1e12);

        vm.prank(admin);
        vm.expectRevert();
        bridge.configureDecimalRatio(0);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configureDecimalRatio(1);
    }

    // ========================================================================
    //                            SENDER TESTS (stake)
    // ========================================================================

    function testStake_Success() public {
        uint256 amount = 500e6;
        string memory receiver = "HN7cABqLq46Es1jh92dQQisAq662SmxELLLsHHe4YWrH";

        vm.roll(42);
        uint256 balBefore = usdc.balanceOf(user1);

        vm.expectEmit(true, true, false, true, address(bridge));
        emit StakeEvent(
            bytes32(uint256(uint160(address(bridge)))),
            peerContract,
            sourceChainId,
            42,
            uint64(amount),
            bytes32(uint256(uint160(user1))),
            receiver,
            1
        );

        vm.prank(user1);
        uint64 nonce = bridge.stake(amount, receiver);

        assertEq(nonce, 1);
        assertEq(usdc.balanceOf(user1), balBefore - amount);
        assertEq(usdc.balanceOf(address(bridge)), 100_000e6 + amount);
    }

    function testStake_InsufficientBalance() public {
        vm.prank(user1);
        vm.expectRevert();
        bridge.stake(20_000e6, "someReceiver");
    }

    function testStake_NotApproved() public {
        address user3 = makeAddr("user3");
        usdc.mint(user3, 1000e6);

        vm.prank(user3);
        vm.expectRevert();
        bridge.stake(100e6, "someReceiver");
    }

    function testStake_UsdcNotConfigured() public {
        Bridge1024 freshBridge = new Bridge1024(admin);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.UsdcNotConfigured.selector);
        freshBridge.stake(100e6, "someReceiver");
    }

    function testStake_WithDecimalRatio() public {
        vm.startPrank(admin);
        bridge.configureUsdc(address(usdt));
        bridge.configureDecimalRatio(1e12);
        vm.stopPrank();

        uint256 stakeAmount = 100e18;
        uint64 expectedAmount = 100e6;

        vm.roll(50);

        vm.expectEmit(true, true, false, true, address(bridge));
        emit StakeEvent(
            bytes32(uint256(uint160(address(bridge)))),
            peerContract,
            sourceChainId,
            50,
            expectedAmount,
            bytes32(uint256(uint160(user1))),
            "HN7cABqLq46Es1jh92dQQisAq662SmxELLLsHHe4YWrH",
            1
        );

        vm.prank(user1);
        bridge.stake(stakeAmount, "HN7cABqLq46Es1jh92dQQisAq662SmxELLLsHHe4YWrH");
    }

    function testStake_WhenPaused() public {
        vm.prank(admin);
        bridge.pause();

        vm.prank(user1);
        vm.expectRevert(abi.encodeWithSignature("EnforcedPause()"));
        bridge.stake(100e6, "someReceiver");
    }

    function testStake_InvalidReceiverAddress() public {
        vm.prank(user1);
        vm.expectRevert(Bridge1024.InvalidReceiverAddress.selector);
        bridge.stake(100e6, "");

        bytes memory longAddr = new bytes(129);
        for (uint256 i = 0; i < 129; i++) longAddr[i] = "a";

        vm.prank(user1);
        vm.expectRevert(Bridge1024.InvalidReceiverAddress.selector);
        bridge.stake(100e6, string(longAddr));
    }

    // ========================================================================
    //                        RECEIVER TESTS (submitSignature)
    // ========================================================================

    // ---- Relayer management ----

    function testAddRelayer() public {
        (address newRelayer,) = makeAddrAndKey("newRelayer");

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

    function testRotateRelayer() public {
        (address newRelayer,) = makeAddrAndKey("rotated");

        vm.prank(admin);
        bridge.rotateRelayer(relayer2, newRelayer);

        assertFalse(bridge.isRelayer(relayer2));
        assertTrue(bridge.isRelayer(newRelayer));
        assertEq(bridge.getRelayerCount(), 3);
    }

    // ---- Signature submission ----

    function testSubmitSignature_SingleRelayer() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        bytes memory sig = _signEventData(data, relayer1Key);

        vm.expectEmit(true, true, false, false, address(bridge));
        emit SignatureSubmitted(relayer1, 1);

        vm.prank(relayer1);
        bridge.submitSignature(data, sig);

        assertFalse(bridge.processedNonces(1));
        assertEq(usdc.balanceOf(user1), 10_000e6);
    }

    function testSubmitSignature_ReachThreshold() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        bytes memory sig1 = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        bridge.submitSignature(data, sig1);

        uint256 userBalBefore = usdc.balanceOf(user1);

        bytes memory sig2 = _signEventData(data, relayer2Key);
        vm.prank(relayer2);
        vm.expectEmit(true, false, false, true, address(bridge));
        emit TokensUnlocked(1, user1, 100e6);
        bridge.submitSignature(data, sig2);

        assertTrue(bridge.processedNonces(1));
        assertEq(usdc.balanceOf(user1), userBalBefore + 100e6);
    }

    function testSubmitSignature_NonceBitmap() public {
        Bridge1024.StakeEventData memory data3 = _makeEventData(50e6, user1, 3);
        _submitSignaturesToThreshold(data3);
        assertTrue(bridge.processedNonces(3));

        Bridge1024.StakeEventData memory data1 = _makeEventData(60e6, user1, 1);
        _submitSignaturesToThreshold(data1);
        assertTrue(bridge.processedNonces(1));

        assertFalse(bridge.processedNonces(2));
    }

    function testSubmitSignature_ReplayBlocked() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        _submitSignaturesToThreshold(data);
        assertTrue(bridge.processedNonces(1));

        bytes memory sig = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.AlreadyProcessed.selector);
        bridge.submitSignature(data, sig);
    }

    function testSubmitSignature_InvalidSignature() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        (, uint256 wrongKey) = makeAddrAndKey("wrongKey");
        bytes memory badSig = _signEventData(data, wrongKey);

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidSignature.selector);
        bridge.submitSignature(data, badSig);
    }

    function testSubmitSignature_NonWhitelistedRelayer() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        (, uint256 outsiderKey) = makeAddrAndKey("outsider");
        address outsider = vm.addr(outsiderKey);

        bytes memory sig = _signEventData(data, outsiderKey);

        vm.prank(outsider);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.submitSignature(data, sig);
    }

    function testSubmitSignature_WrongSourceContract() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        data.sourceContract = bytes32(uint256(0x1234));

        bytes memory sig = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidSourceContract.selector);
        bridge.submitSignature(data, sig);
    }

    function testSubmitSignature_WrongChainId() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        data.sourceChainId = 999;

        bytes memory sig = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidChainId.selector);
        bridge.submitSignature(data, sig);
    }

    function testSubmitSignature_EventDataMismatch() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        bytes memory sig1 = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        bridge.submitSignature(data, sig1);

        Bridge1024.StakeEventData memory tampered = _makeEventData(100e6, user1, 1);
        tampered.sender = bytes32(uint256(uint160(user2)));
        bytes memory sig2 = _signEventData(tampered, relayer2Key);

        vm.prank(relayer2);
        vm.expectRevert(Bridge1024.InvalidEventData.selector);
        bridge.submitSignature(tampered, sig2);
    }

    function testSubmitSignature_EventDataMismatch_Amount() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        bytes memory sig1 = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        bridge.submitSignature(data, sig1);

        Bridge1024.StakeEventData memory tampered = _makeEventData(200e6, user1, 1);
        bytes memory sig2 = _signEventData(tampered, relayer2Key);

        vm.prank(relayer2);
        vm.expectRevert(Bridge1024.InvalidEventData.selector);
        bridge.submitSignature(tampered, sig2);
    }

    function testSubmitSignature_FrozenThreshold() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        bytes memory sig1 = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        bridge.submitSignature(data, sig1);

        (address relayer4,) = makeAddrAndKey("relayer4");
        vm.prank(admin);
        bridge.addRelayer(relayer4);
        assertEq(bridge.getRelayerCount(), 4);

        uint256 userBalBefore = usdc.balanceOf(user1);

        bytes memory sig2 = _signEventData(data, relayer2Key);
        vm.prank(relayer2);
        vm.expectEmit(true, false, false, true, address(bridge));
        emit TokensUnlocked(1, user1, 100e6);
        bridge.submitSignature(data, sig2);

        assertEq(usdc.balanceOf(user1), userBalBefore + 100e6);
    }

    function testSubmitSignature_WhenPaused() public {
        vm.prank(admin);
        bridge.pause();

        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        bytes memory sig = _signEventData(data, relayer1Key);

        vm.prank(relayer1);
        vm.expectRevert(abi.encodeWithSignature("EnforcedPause()"));
        bridge.submitSignature(data, sig);
    }

    function testSubmitSignature_CanonicalS() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        bytes memory highSSig = _signEventDataHighS(data, relayer1Key);

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidSignature.selector);
        bridge.submitSignature(data, highSSig);
    }

    function testSubmitSignature_DuplicateRelayerSig() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        bytes memory sig = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        bridge.submitSignature(data, sig);

        bytes memory sigAgain = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.RelayerAlreadySigned.selector);
        bridge.submitSignature(data, sigAgain);
    }

    // ========================================================================
    //                          RATE LIMITING TESTS
    // ========================================================================

    function testRateLimit_ExceedWindow() public {
        vm.prank(admin);
        bridge.configureRateLimits(500e6, 3600, type(uint64).max, 0);

        Bridge1024.StakeEventData memory data1 = _makeEventData(400e6, user1, 1);
        _submitSignaturesToThreshold(data1);

        Bridge1024.StakeEventData memory data2 = _makeEventData(200e6, user1, 2);
        bytes memory sig1 = _signEventData(data2, relayer1Key);
        vm.prank(relayer1);
        bridge.submitSignature(data2, sig1);

        bytes memory sig2 = _signEventData(data2, relayer2Key);
        vm.prank(relayer2);
        vm.expectRevert(Bridge1024.RateLimitExceeded.selector);
        bridge.submitSignature(data2, sig2);
    }

    function testRateLimit_SlidingWindow() public {
        vm.prank(admin);
        bridge.configureRateLimits(500e6, 3600, type(uint64).max, 0);

        Bridge1024.StakeEventData memory data1 = _makeEventData(400e6, user1, 1);
        _submitSignaturesToThreshold(data1);

        vm.warp(block.timestamp + 3601);

        Bridge1024.StakeEventData memory data2 = _makeEventData(400e6, user1, 2);
        _submitSignaturesToThreshold(data2);

        assertTrue(bridge.processedNonces(1));
        assertTrue(bridge.processedNonces(2));
    }

    function testRateLimit_MaxSingle() public {
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, 500e6, 0);

        Bridge1024.StakeEventData memory data = _makeEventData(600e6, user1, 1);
        bytes memory sig1 = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        bridge.submitSignature(data, sig1);

        bytes memory sig2 = _signEventData(data, relayer2Key);
        vm.prank(relayer2);
        vm.expectRevert(Bridge1024.SingleTransferExceeded.selector);
        bridge.submitSignature(data, sig2);
    }

    function testRateLimit_MinReserve() public {
        vm.prank(admin);
        bridge.configureRateLimits(type(uint64).max, 3600, type(uint64).max, 99_500e6);

        Bridge1024.StakeEventData memory data = _makeEventData(1000e6, user1, 1);
        bytes memory sig1 = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        bridge.submitSignature(data, sig1);

        bytes memory sig2 = _signEventData(data, relayer2Key);
        vm.prank(relayer2);
        vm.expectRevert(Bridge1024.InsufficientReserve.selector);
        bridge.submitSignature(data, sig2);
    }

    // ========================================================================
    //                            ADMIN TESTS
    // ========================================================================

    function testProposeAdmin() public {
        vm.expectEmit(true, true, false, false, address(bridge));
        emit AdminTransferProposed(admin, user1);

        vm.prank(admin);
        bridge.proposeAdmin(user1);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.proposeAdmin(user2);
    }

    function testAcceptAdmin() public {
        vm.prank(admin);
        bridge.proposeAdmin(user1);

        vm.expectEmit(true, true, false, false, address(bridge));
        emit AdminTransferAccepted(admin, user1);

        vm.prank(user1);
        bridge.acceptAdmin();

        (, address newAdm,,,,,,,) = bridge.senderState();
        assertEq(newAdm, user1);

        vm.prank(admin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configureUsdc(address(usdc));
    }

    function testAcceptAdmin_NotPending() public {
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.acceptAdmin();
    }

    function testPause_Unpause() public {
        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.pause();

        vm.prank(admin);
        bridge.pause();

        vm.prank(user1);
        vm.expectRevert(abi.encodeWithSignature("EnforcedPause()"));
        bridge.stake(100e6, "someReceiver");

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.unpause();

        vm.prank(admin);
        bridge.unpause();

        vm.prank(user1);
        bridge.stake(100e6, "someReceiver");
    }

    function testEmergencyWithdraw() public {
        uint256 bridgeBal = usdc.balanceOf(address(bridge));
        uint256 adminBal = usdc.balanceOf(admin);

        vm.prank(admin);
        bridge.emergencyWithdraw(address(usdc), bridgeBal, admin);

        assertEq(usdc.balanceOf(address(bridge)), 0);
        assertEq(usdc.balanceOf(admin), adminBal + bridgeBal);

        vm.prank(user1);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.emergencyWithdraw(address(usdc), 1, user1);
    }

    // ========================================================================
    //                           SECURITY TESTS
    // ========================================================================

    function testReplayAttack() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);
        _submitSignaturesToThreshold(data);
        assertTrue(bridge.processedNonces(1));

        bytes memory replaySig = _signEventData(data, relayer1Key);
        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.AlreadyProcessed.selector);
        bridge.submitSignature(data, replaySig);
    }

    function testSignatureForgery() public {
        Bridge1024.StakeEventData memory data = _makeEventData(100e6, user1, 1);

        (, uint256 attackerKey) = makeAddrAndKey("attacker");
        bytes memory forgedSig = _signEventData(data, attackerKey);

        vm.prank(relayer1);
        vm.expectRevert(Bridge1024.InvalidSignature.selector);
        bridge.submitSignature(data, forgedSig);
    }

    function testAccessControl() public {
        vm.startPrank(user1);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configureUsdc(address(usdc));

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configurePeer(peerContract, 1, 2);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configureDecimalRatio(1);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.addRelayer(user2);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.removeRelayer(relayer1);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.rotateRelayer(relayer1, user2);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.proposeAdmin(user2);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.pause();

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.unpause();

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.emergencyWithdraw(address(usdc), 1, user1);

        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configureRateLimits(1, 1, 1, 1);

        vm.stopPrank();
    }
}
