// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";

contract Bridge1024 is Pausable, ReentrancyGuard {
    using SafeERC20 for IERC20;
    using SafeCast for uint256;

    uint8 public constant MAX_RELAYERS = 18;
    uint256 private constant SECP256K1_N_HALF =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    error Unauthorized();
    error ZeroAddress();
    error UsdcNotConfigured();
    error InvalidReceiverAddress();
    error RelayerAlreadyExists();
    error TooManyRelayers();
    error RelayerNotFound();
    error AlreadyProcessed();
    error InvalidSignature();
    error InvalidSourceContract();
    error InvalidChainId();
    error InvalidEventData();
    error RateLimitExceeded();
    error SingleTransferExceeded();
    error InsufficientReserve();
    error RelayerAlreadySigned();
    error ZeroAmount();
    error ZeroRatio();

    struct StakeEventData {
        bytes32 sourceContract;
        bytes32 targetContract;
        uint64 sourceChainId;
        uint64 targetChainId;
        uint64 blockHeight;
        uint64 amount;
        bytes32 sender;
        string receiverAddress;
        uint64 nonce;
    }

    struct NonceSignature {
        mapping(address => bool) signedRelayers;
        uint8 signatureCount;
        bool isUnlocked;
        bool isInitialized;
        uint8 frozenThreshold;
        StakeEventData eventData;
    }

    struct SenderState {
        address vault;
        address admin;
        address usdcContract;
        uint64 nonce;
        bytes32 targetContract;
        uint64 sourceChainId;
        uint64 targetChainId;
        address pendingAdmin;
        uint64 decimalRatio;
    }

    struct ReceiverState {
        address vault;
        address admin;
        address usdcContract;
        uint64 relayerCount;
        bytes32 sourceContract;
        uint64 sourceChainId;
        uint64 targetChainId;
        address pendingAdmin;
        uint64 decimalRatio;
        address[] relayers;
    }

    SenderState public senderState;
    ReceiverState public receiverState;

    uint256 public maxUnlockPerWindow;
    uint256 public windowDuration;
    uint256 public currentWindowStart;
    uint256 public currentWindowUsage;
    uint256 public previousWindowUsage;
    uint256 public maxSingleUnlock;
    uint256 public minimumReserve;

    mapping(uint64 => bool) public processedNonces;
    mapping(uint64 => NonceSignature) public nonceSignatures;

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

    modifier onlyAdmin() {
        if (msg.sender != senderState.admin && msg.sender != receiverState.admin) {
            revert Unauthorized();
        }
        _;
    }

    modifier onlyWhitelistedRelayer() {
        if (!isRelayer(msg.sender)) revert Unauthorized();
        _;
    }

    constructor(address adminAddress) {
        if (adminAddress == address(0)) revert ZeroAddress();
        senderState.admin = adminAddress;
        senderState.vault = address(this);
        senderState.decimalRatio = 1;
        receiverState.admin = adminAddress;
        receiverState.vault = address(this);
        receiverState.decimalRatio = 1;
    }

    // ─── Admin Functions ────────────────────────────────────────────────

    function configureUsdc(address usdcAddress) external onlyAdmin {
        if (usdcAddress == address(0)) revert ZeroAddress();
        senderState.usdcContract = usdcAddress;
        receiverState.usdcContract = usdcAddress;
    }

    function configurePeer(
        bytes32 peerContract,
        uint64 sourceChainId,
        uint64 targetChainId
    ) external onlyAdmin {
        senderState.targetContract = peerContract;
        senderState.sourceChainId = sourceChainId;
        senderState.targetChainId = targetChainId;
        receiverState.sourceContract = peerContract;
        receiverState.sourceChainId = targetChainId;
        receiverState.targetChainId = sourceChainId;
    }

    function configureDecimalRatio(uint64 ratio) external onlyAdmin {
        if (ratio == 0) revert ZeroRatio();
        senderState.decimalRatio = ratio;
        receiverState.decimalRatio = ratio;
    }

    function configureRateLimits(
        uint256 _maxPerWindow,
        uint256 _windowDuration,
        uint256 _maxSingle,
        uint256 _minReserve
    ) external onlyAdmin {
        maxUnlockPerWindow = _maxPerWindow;
        windowDuration = _windowDuration;
        maxSingleUnlock = _maxSingle;
        minimumReserve = _minReserve;
    }

    function addRelayer(address relayerAddress) external onlyAdmin {
        if (relayerAddress == address(0)) revert ZeroAddress();
        if (receiverState.relayers.length >= MAX_RELAYERS) revert TooManyRelayers();
        for (uint256 i = 0; i < receiverState.relayers.length; i++) {
            if (receiverState.relayers[i] == relayerAddress) revert RelayerAlreadyExists();
        }
        receiverState.relayers.push(relayerAddress);
        receiverState.relayerCount = uint64(receiverState.relayers.length);
        emit RelayerAdded(relayerAddress);
    }

    function removeRelayer(address relayerAddress) external onlyAdmin {
        uint256 idx = type(uint256).max;
        for (uint256 i = 0; i < receiverState.relayers.length; i++) {
            if (receiverState.relayers[i] == relayerAddress) {
                idx = i;
                break;
            }
        }
        if (idx == type(uint256).max) revert RelayerNotFound();
        receiverState.relayers[idx] = receiverState.relayers[receiverState.relayers.length - 1];
        receiverState.relayers.pop();
        receiverState.relayerCount = uint64(receiverState.relayers.length);
        emit RelayerRemoved(relayerAddress);
    }

    function rotateRelayer(address oldRelayer, address newRelayer) external onlyAdmin {
        if (newRelayer == address(0)) revert ZeroAddress();
        uint256 idx = type(uint256).max;
        for (uint256 i = 0; i < receiverState.relayers.length; i++) {
            if (receiverState.relayers[i] == oldRelayer) idx = i;
            if (receiverState.relayers[i] == newRelayer) revert RelayerAlreadyExists();
        }
        if (idx == type(uint256).max) revert RelayerNotFound();
        receiverState.relayers[idx] = newRelayer;
        emit RelayerRemoved(oldRelayer);
        emit RelayerAdded(newRelayer);
    }

    function proposeAdmin(address newAdmin) external onlyAdmin {
        if (newAdmin == address(0)) revert ZeroAddress();
        address currentAdmin = senderState.admin;
        senderState.pendingAdmin = newAdmin;
        receiverState.pendingAdmin = newAdmin;
        emit AdminTransferProposed(currentAdmin, newAdmin);
    }

    function acceptAdmin() external {
        if (msg.sender != senderState.pendingAdmin) revert Unauthorized();
        address oldAdmin = senderState.admin;
        senderState.admin = msg.sender;
        receiverState.admin = msg.sender;
        senderState.pendingAdmin = address(0);
        receiverState.pendingAdmin = address(0);
        emit AdminTransferAccepted(oldAdmin, msg.sender);
    }

    function pause() external onlyAdmin {
        _pause();
    }

    function unpause() external onlyAdmin {
        _unpause();
    }

    function emergencyWithdraw(address token, uint256 amount, address to) external onlyAdmin {
        if (to == address(0)) revert ZeroAddress();
        IERC20(token).safeTransfer(to, amount);
    }

    // ─── View Functions ─────────────────────────────────────────────────

    function getRelayerCount() external view returns (uint256) {
        return receiverState.relayers.length;
    }

    function isRelayer(address addr) public view returns (bool) {
        for (uint256 i = 0; i < receiverState.relayers.length; i++) {
            if (receiverState.relayers[i] == addr) return true;
        }
        return false;
    }

    // ─── Public Functions ───────────────────────────────────────────────

    function stake(
        uint256 amount,
        string memory receiverAddress
    ) external whenNotPaused nonReentrant returns (uint64) {
        if (senderState.usdcContract == address(0)) revert UsdcNotConfigured();
        if (amount == 0) revert ZeroAmount();
        _validateReceiverAddress(receiverAddress);

        IERC20 usdc = IERC20(senderState.usdcContract);
        uint256 balanceBefore = usdc.balanceOf(address(this));
        usdc.safeTransferFrom(msg.sender, address(this), amount);
        uint256 balanceAfter = usdc.balanceOf(address(this));
        uint256 actualAmount = balanceAfter - balanceBefore;

        senderState.nonce++;
        uint64 currentNonce = senderState.nonce;

        uint64 convertedAmount = (actualAmount / senderState.decimalRatio).toUint64();
        if (convertedAmount == 0) revert ZeroAmount();

        bytes32 sender = bytes32(uint256(uint160(msg.sender)));

        emit StakeEvent(
            bytes32(uint256(uint160(address(this)))),
            senderState.targetContract,
            senderState.sourceChainId,
            uint64(block.number),
            convertedAmount,
            sender,
            receiverAddress,
            currentNonce
        );

        return currentNonce;
    }

    function submitSignature(
        StakeEventData memory eventData,
        bytes memory signature
    ) external onlyWhitelistedRelayer whenNotPaused nonReentrant {
        if (receiverState.usdcContract == address(0)) revert UsdcNotConfigured();
        if (eventData.sourceContract != receiverState.sourceContract) revert InvalidSourceContract();
        if (eventData.sourceChainId != receiverState.sourceChainId) revert InvalidChainId();
        if (processedNonces[eventData.nonce]) revert AlreadyProcessed();

        NonceSignature storage nonceSig = nonceSignatures[eventData.nonce];

        if (nonceSig.signatureCount == 0) {
            nonceSig.eventData = eventData;
            nonceSig.isInitialized = true;
            uint64 rc = receiverState.relayerCount;
            nonceSig.frozenThreshold = uint8((rc * 2 + 2) / 3);
        } else {
            _verifyEventDataConsistency(nonceSig.eventData, eventData);
        }

        if (nonceSig.signedRelayers[msg.sender]) revert RelayerAlreadySigned();
        _verifyEcdsaSignature(eventData, signature, msg.sender);

        nonceSig.signedRelayers[msg.sender] = true;
        nonceSig.signatureCount++;

        emit SignatureSubmitted(msg.sender, eventData.nonce);

        if (nonceSig.signatureCount >= nonceSig.frozenThreshold && !nonceSig.isUnlocked) {
            nonceSig.isUnlocked = true;
            processedNonces[eventData.nonce] = true;

            uint256 unlockAmount = uint256(eventData.amount) * receiverState.decimalRatio;

            _checkRateLimit(unlockAmount);
            if (maxSingleUnlock != 0 && unlockAmount > maxSingleUnlock) {
                revert SingleTransferExceeded();
            }
            _checkVaultInvariant(unlockAmount);

            address receiver = _parseAddress(eventData.receiverAddress);
            IERC20(receiverState.usdcContract).safeTransfer(receiver, unlockAmount);

            emit TokensUnlocked(eventData.nonce, receiver, eventData.amount);
        }
    }

    // ─── Internal Functions ─────────────────────────────────────────────

    function _verifyEventDataConsistency(
        StakeEventData storage stored,
        StakeEventData memory submitted
    ) internal view {
        if (stored.sourceContract != submitted.sourceContract) revert InvalidEventData();
        if (stored.targetContract != submitted.targetContract) revert InvalidEventData();
        if (stored.sourceChainId != submitted.sourceChainId) revert InvalidEventData();
        if (stored.targetChainId != submitted.targetChainId) revert InvalidEventData();
        if (stored.blockHeight != submitted.blockHeight) revert InvalidEventData();
        if (stored.amount != submitted.amount) revert InvalidEventData();
        if (stored.sender != submitted.sender) revert InvalidEventData();
        if (
            keccak256(bytes(stored.receiverAddress)) !=
            keccak256(bytes(submitted.receiverAddress))
        ) revert InvalidEventData();
        if (stored.nonce != submitted.nonce) revert InvalidEventData();
    }

    function _checkRateLimit(uint256 amount) internal {
        if (maxUnlockPerWindow == 0 || windowDuration == 0) return;

        uint256 currentTime = block.timestamp;

        if (currentTime >= currentWindowStart + windowDuration) {
            if (currentTime < currentWindowStart + 2 * windowDuration) {
                previousWindowUsage = currentWindowUsage;
            } else {
                previousWindowUsage = 0;
            }
            currentWindowUsage = 0;
            currentWindowStart = currentTime;
        }

        uint256 elapsed = currentTime - currentWindowStart;
        uint256 remainingWeight = windowDuration - elapsed;
        uint256 slidingUsage =
            (previousWindowUsage * remainingWeight / windowDuration) + currentWindowUsage;

        if (slidingUsage + amount > maxUnlockPerWindow) revert RateLimitExceeded();

        currentWindowUsage += amount;
    }

    function _checkVaultInvariant(uint256 unlockAmount) internal view {
        uint256 vaultBalance = IERC20(receiverState.usdcContract).balanceOf(address(this));
        if (vaultBalance < unlockAmount + minimumReserve) revert InsufficientReserve();
    }

    function _verifyEcdsaSignature(
        StakeEventData memory eventData,
        bytes memory signature,
        address expectedSigner
    ) internal pure {
        if (signature.length != 65) revert InvalidSignature();

        bytes32 dataHash = _hashEventData(eventData);
        bytes32 ethSignedHash =
            keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", dataHash));

        bytes32 r;
        bytes32 s;
        uint8 v;
        assembly {
            r := mload(add(signature, 32))
            s := mload(add(signature, 64))
            v := byte(0, mload(add(signature, 96)))
        }

        if (uint256(s) > SECP256K1_N_HALF) revert InvalidSignature();
        if (v < 27) v += 27;
        if (v != 27 && v != 28) revert InvalidSignature();

        address recovered = ecrecover(ethSignedHash, v, r, s);
        if (recovered == address(0)) revert InvalidSignature();
        if (recovered != expectedSigner) revert InvalidSignature();
    }

    function _hashEventData(StakeEventData memory data) internal pure returns (bytes32) {
        bytes memory part1 = abi.encodePacked(
            '{"sourceContract":"',
            _bytes32ToHex(data.sourceContract),
            '","targetContract":"',
            _bytes32ToHex(data.targetContract),
            '","chainId":"',
            _uint64ToString(data.sourceChainId),
            '","blockHeight":"',
            _uint64ToString(data.blockHeight)
        );
        bytes memory part2 = abi.encodePacked(
            '","amount":"',
            _uint64ToString(data.amount),
            '","sender":"',
            _bytes32ToHex(data.sender),
            '","receiverAddress":"',
            data.receiverAddress,
            '","nonce":"',
            _uint64ToString(data.nonce),
            '"}'
        );
        return sha256(abi.encodePacked(part1, part2));
    }

    function _bytes32ToHex(bytes32 value) internal pure returns (string memory) {
        bytes memory alphabet = "0123456789abcdef";
        bytes memory result = new bytes(64);
        for (uint256 i = 0; i < 32; i++) {
            result[i * 2] = alphabet[uint8(value[i]) >> 4];
            result[i * 2 + 1] = alphabet[uint8(value[i]) & 0x0f];
        }
        return string(result);
    }

    function _uint64ToString(uint64 value) internal pure returns (string memory) {
        if (value == 0) return "0";
        uint64 temp = value;
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

    function _parseAddress(string memory addr) internal pure returns (address) {
        bytes memory b = bytes(addr);
        uint256 start;
        uint256 expectedLen;
        if (b.length == 42 && b[0] == "0" && (b[1] == "x" || b[1] == "X")) {
            start = 2;
            expectedLen = 42;
        } else if (b.length == 40) {
            start = 0;
            expectedLen = 40;
        } else {
            revert InvalidReceiverAddress();
        }
        uint160 result = 0;
        for (uint256 i = start; i < expectedLen; i++) {
            uint8 c = uint8(b[i]);
            uint8 digit;
            if (c >= 48 && c <= 57) {
                digit = c - 48;
            } else if (c >= 65 && c <= 70) {
                digit = c - 55;
            } else if (c >= 97 && c <= 102) {
                digit = c - 87;
            } else {
                revert InvalidReceiverAddress();
            }
            result = result * 16 + uint160(digit);
        }
        return address(result);
    }

    function _validateReceiverAddress(string memory addr) internal pure {
        bytes memory b = bytes(addr);
        if (b.length == 0) revert InvalidReceiverAddress();
        if (b.length > 128) revert InvalidReceiverAddress();
        for (uint256 i = 0; i < b.length; i++) {
            uint8 c = uint8(b[i]);
            if (
                !((c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122))
            ) revert InvalidReceiverAddress();
        }
    }
}
