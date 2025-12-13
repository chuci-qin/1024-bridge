// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/utils/math/SafeCast.sol";

/**
 * @title Bridge1024
 * @notice Cross-chain bridge between EVM and SVM (1024chain)
 * @dev Implements stake-unlock mechanism with multi-signature verification
 */
contract Bridge1024 {
    
    // ============ State Variables ============
    
    struct SenderState {
        address vault;
        address admin;
        address usdcContract;
        uint64 nonce;
        bytes32 targetContract;
        uint64 sourceChainId;
        uint64 targetChainId;
    }
    
    struct ReceiverState {
        address vault;
        address admin;
        address usdcContract;
        uint64 relayerCount;
        bytes32 sourceContract;
        uint64 sourceChainId;
        uint64 targetChainId;
        address[] relayers;
        uint64 lastNonce;
    }
    
    struct StakeEventData {
        bytes32 sourceContract;
        bytes32 targetContract;
        uint64 sourceChainId;
        uint64 targetChainId;
        uint64 blockHeight;
        uint64 amount;
        address sender;           // EVM 发起者地址
        string receiverAddress;   // Solana 接收地址
        uint64 nonce;
    }
    
    struct NonceSignature {
        mapping(address => bool) signedRelayers;
        uint8 signatureCount;
        bool isUnlocked;
        StakeEventData eventData;
    }
    
    SenderState public senderState;
    ReceiverState private receiverStateInternal;
    
    mapping(uint64 => NonceSignature) public nonceSignatures;
    
    // Getter functions for receiver state (because of nested mapping)
    function receiverState() external view returns (
        address vault,
        address admin,
        address usdcContract,
        uint64 relayerCount,
        bytes32 sourceContract,
        uint64 sourceChainId,
        uint64 targetChainId,
        uint64 lastNonce
    ) {
        return (
            receiverStateInternal.vault,
            receiverStateInternal.admin,
            receiverStateInternal.usdcContract,
            receiverStateInternal.relayerCount,
            receiverStateInternal.sourceContract,
            receiverStateInternal.sourceChainId,
            receiverStateInternal.targetChainId,
            receiverStateInternal.lastNonce
        );
    }
    
    function getRelayers() external view returns (address[] memory) {
        return receiverStateInternal.relayers;
    }
    
    uint256 public constant MAX_RELAYERS = 18;
    
    // ============ Events ============
    
    event StakeEvent(
        bytes32 indexed sourceContract,
        bytes32 indexed targetContract,
        uint64 chainId,
        uint64 blockHeight,
        uint64 amount,
        address sender,           // EVM 发起者地址（用于追踪和映射）
        string receiverAddress,   // Solana 接收地址（用于接收 USDC）
        uint64 nonce
    );
    
    event RelayerAdded(address indexed relayer);
    event RelayerRemoved(address indexed relayer);
    event SignatureSubmitted(address indexed relayer, uint64 indexed nonce);
    event TokensUnlocked(uint64 indexed nonce, address receiver, uint64 amount);
    
    // ============ Errors ============
    
    error Unauthorized();
    error UsdcNotConfigured();
    error InsufficientBalance();
    error RelayerAlreadyExists();
    error RelayerNotFound();
    error InvalidNonce();
    error InvalidSignature();
    error InvalidSourceContract();
    error InvalidChainId();
    error TooManyRelayers();
    error RelayerAlreadySigned();
    error AlreadyInitialized();
    error InvalidEventData();
    
    // ============ Modifiers ============
    
    modifier onlyAdmin() {
        if (msg.sender != senderState.admin) revert Unauthorized();
        _;
    }
    
    modifier onlyWhitelistedRelayer() {
        bool isWhitelisted = false;
        for (uint i = 0; i < receiverStateInternal.relayers.length; i++) {
            if (receiverStateInternal.relayers[i] == msg.sender) {
                isWhitelisted = true;
                break;
            }
        }
        if (!isWhitelisted) revert Unauthorized();
        _;
    }
    
    // ============ Initialization Functions ============
    
    /**
     * @notice Initialize both sender and receiver contracts
     * @param adminAddress Admin address for management operations
     */
    function initialize(address adminAddress) external {
        if (senderState.admin != address(0)) revert AlreadyInitialized();
        
        // Initialize sender state - contract itself acts as vault
        senderState.vault = address(this);
        senderState.admin = adminAddress;
        senderState.nonce = 0;
        senderState.usdcContract = address(0);
        senderState.targetContract = bytes32(0);
        senderState.sourceChainId = 0;
        senderState.targetChainId = 0;
        
        // Initialize receiver state - contract itself acts as vault
        receiverStateInternal.vault = address(this);
        receiverStateInternal.admin = adminAddress;
        receiverStateInternal.lastNonce = 0;
        receiverStateInternal.relayerCount = 0;
        receiverStateInternal.usdcContract = address(0);
        receiverStateInternal.sourceContract = bytes32(0);
        receiverStateInternal.sourceChainId = 0;
        receiverStateInternal.targetChainId = 0;
    }
    
    /**
     * @notice Configure USDC token address
     * @param usdcAddress USDC ERC20 contract address
     */
    function configureUsdc(address usdcAddress) external onlyAdmin {
        senderState.usdcContract = usdcAddress;
        receiverStateInternal.usdcContract = usdcAddress;
    }
    
    /**
     * @notice Configure peer contract and chain IDs
     * @param peerContract Peer contract identifier on the other chain (bytes32 to support cross-chain addresses)
     * @param sourceChainId Current chain ID
     * @param targetChainId Target chain ID
     */
    function configurePeer(
        bytes32 peerContract,
        uint64 sourceChainId,
        uint64 targetChainId
    ) external onlyAdmin {
        // Configure sender state (for EVM → peer transfers)
        senderState.targetContract = peerContract;
        senderState.sourceChainId = sourceChainId;  // EVM chain ID
        senderState.targetChainId = targetChainId;  // Peer chain ID
        
        // Configure receiver state (for peer → EVM transfers)
        // Note: Chain IDs are swapped for receiver since it receives from peer
        receiverStateInternal.sourceContract = peerContract;
        receiverStateInternal.sourceChainId = targetChainId;  // Peer chain ID (source of incoming events)
        receiverStateInternal.targetChainId = sourceChainId;  // EVM chain ID (target of incoming events)
    }
    
    // ============ Sender Functions ============
    
    /**
     * @notice Stake USDC tokens to initiate cross-chain transfer
     * @param amount Amount of USDC to stake
     * @param receiverAddress Receiver address on target chain
     * @return nonce The nonce for this stake transaction
     */
    function stake(uint256 amount, string memory receiverAddress) external returns (uint64) {
        if (senderState.usdcContract == address(0)) revert UsdcNotConfigured();
        
        // Transfer tokens from user to contract (contract acts as vault)
        IERC20 usdc = IERC20(senderState.usdcContract);
        require(usdc.transferFrom(msg.sender, address(this), amount), "Transfer failed");
        
        // Update nonce
        uint64 currentNonce = senderState.nonce;
        uint64 newNonce = currentNonce + 1;
        if (newNonce == 0 && currentNonce != type(uint64).max) {
            revert InvalidNonce();
        }
        senderState.nonce = newNonce;
        
        // Emit stake event
        // Use SafeCast to ensure amount fits in uint64 without truncation
        emit StakeEvent(
            bytes32(uint256(uint160(address(this)))),
            senderState.targetContract,
            senderState.sourceChainId,
            SafeCast.toUint64(block.number),
            SafeCast.toUint64(amount),
            msg.sender,       // 添加：EVM 发起者地址
            receiverAddress,  // Solana 接收地址
            newNonce
        );
        
        return newNonce;
    }
    
    // ============ Receiver Functions ============
    
    /**
     * @notice Add a relayer to whitelist
     * @param relayerAddress Relayer address
     */
    function addRelayer(address relayerAddress) external onlyAdmin {
        // Check if relayer already exists
        for (uint i = 0; i < receiverStateInternal.relayers.length; i++) {
            if (receiverStateInternal.relayers[i] == relayerAddress) revert RelayerAlreadyExists();
        }
        
        // Check max relayers limit
        if (receiverStateInternal.relayers.length >= MAX_RELAYERS) revert TooManyRelayers();
        
        // Add relayer
        receiverStateInternal.relayers.push(relayerAddress);
        receiverStateInternal.relayerCount++;
        
        emit RelayerAdded(relayerAddress);
    }
    
    /**
     * @notice Remove a relayer from whitelist
     * @param relayerAddress Relayer address to remove
     */
    function removeRelayer(address relayerAddress) external onlyAdmin {
        bool found = false;
        uint256 index = 0;
        
        for (uint i = 0; i < receiverStateInternal.relayers.length; i++) {
            if (receiverStateInternal.relayers[i] == relayerAddress) {
                found = true;
                index = i;
                break;
            }
        }
        
        if (!found) revert RelayerNotFound();
        
        // Remove relayer
        receiverStateInternal.relayers[index] = receiverStateInternal.relayers[receiverStateInternal.relayers.length - 1];
        receiverStateInternal.relayers.pop();
        receiverStateInternal.relayerCount--;
        
        emit RelayerRemoved(relayerAddress);
    }
    
    /**
     * @notice Submit signature for cross-chain request
     * @param eventData Stake event data from source chain
     * @param signature ECDSA signature
     */
    function submitSignature(
        StakeEventData memory eventData,
        bytes memory signature
    ) external onlyWhitelistedRelayer {
        if (receiverStateInternal.usdcContract == address(0)) revert UsdcNotConfigured();
        
        // Verify source contract address
        if (eventData.sourceContract != receiverStateInternal.sourceContract) revert InvalidSourceContract();
        
        // Verify chain ID
        if (eventData.sourceChainId != receiverStateInternal.sourceChainId) revert InvalidChainId();
        
        // Verify nonce is incrementing
        if (eventData.nonce <= receiverStateInternal.lastNonce) revert InvalidNonce();
        
        // Initialize nonce signature if first signature
        NonceSignature storage nonceSignature = nonceSignatures[eventData.nonce];
        if (nonceSignature.signatureCount == 0) {
            nonceSignature.eventData = eventData;
            nonceSignature.isUnlocked = false;
        } else {
            if (nonceSignature.signedRelayers[msg.sender]) revert RelayerAlreadySigned();
            
            // Verify that the submitted eventData matches the stored eventData
            // This prevents a malicious relayer from submitting different eventData
            if (
                nonceSignature.eventData.sourceContract != eventData.sourceContract ||
                nonceSignature.eventData.targetContract != eventData.targetContract ||
                nonceSignature.eventData.sourceChainId != eventData.sourceChainId ||
                nonceSignature.eventData.targetChainId != eventData.targetChainId ||
                nonceSignature.eventData.blockHeight != eventData.blockHeight ||
                nonceSignature.eventData.amount != eventData.amount ||
                keccak256(bytes(nonceSignature.eventData.receiverAddress)) != keccak256(bytes(eventData.receiverAddress)) ||
                nonceSignature.eventData.nonce != eventData.nonce
            ) {
                revert InvalidEventData();
            }
        }
        
        // Verify ECDSA signature
        _verifyEcdsaSignature(eventData, signature, msg.sender);
        
        // Record signature
        nonceSignature.signedRelayers[msg.sender] = true;
        nonceSignature.signatureCount++;
        
        emit SignatureSubmitted(msg.sender, eventData.nonce);
        
        // Calculate threshold: ceil(relayerCount * 2 / 3)
        uint8 threshold = uint8((receiverStateInternal.relayerCount * 2 + 2) / 3);
        
        // Check if threshold is reached
        if (nonceSignature.signatureCount >= threshold && !nonceSignature.isUnlocked) {
            nonceSignature.isUnlocked = true;
            // Use the stored eventData.nonce instead of function parameter
            receiverStateInternal.lastNonce = nonceSignature.eventData.nonce;
            
            // Unlock tokens: transfer from contract (vault) to receiver
            // Use the stored eventData instead of function parameter to prevent inconsistencies
            IERC20 usdc = IERC20(receiverStateInternal.usdcContract);
            address receiver = _parseAddress(nonceSignature.eventData.receiverAddress);
            require(usdc.transfer(receiver, nonceSignature.eventData.amount), "Transfer failed");
            
            emit TokensUnlocked(nonceSignature.eventData.nonce, receiver, nonceSignature.eventData.amount);
        }
    }
    
    // ============ View Functions ============
    
    /**
     * @notice Check if address is a whitelisted relayer
     * @param relayerAddress Address to check
     * @return bool True if address is whitelisted
     */
    function isRelayer(address relayerAddress) external view returns (bool) {
        for (uint i = 0; i < receiverStateInternal.relayers.length; i++) {
            if (receiverStateInternal.relayers[i] == relayerAddress) return true;
        }
        return false;
    }
    
    /**
     * @notice Get total number of relayers
     * @return uint256 Number of relayers
     */
    function getRelayerCount() external view returns (uint256) {
        return receiverStateInternal.relayerCount;
    }
    
    /**
     * @notice Get sender nonce
     * @return uint64 Current sender nonce
     */
    function getSenderNonce() external view returns (uint64) {
        return senderState.nonce;
    }
    
    /**
     * @notice Get receiver last nonce
     * @return uint64 Last processed nonce
     */
    function getReceiverLastNonce() external view returns (uint64) {
        return receiverStateInternal.lastNonce;
    }
    
    // ============ Internal Functions ============
    
    /**
     * @notice Verify ECDSA signature (aligned with SVM implementation)
     * @dev Hash algorithm: SHA-256(JSON.stringify(eventData))
     * @dev Signature format: EIP-191 standard
     */
    function _verifyEcdsaSignature(
        StakeEventData memory eventData,
        bytes memory signature,
        address signer
    ) internal view {
        // Signature must be 65 bytes (r, s, v)
        if (signature.length != 65) revert InvalidSignature();
        
        // Hash event data using SHA-256 and JSON format (aligned with SVM)
        bytes32 hash = _hashEventData(eventData);
        
        // Apply EIP-191 prefix
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", hash));
        
        // Extract v, r, s
        bytes32 r;
        bytes32 s;
        uint8 v;
        assembly {
            r := mload(add(signature, 32))
            s := mload(add(signature, 64))
            v := byte(0, mload(add(signature, 96)))
        }
        
        // Adjust v if necessary
        if (v < 27) v += 27;
        if (v != 27 && v != 28) revert InvalidSignature();
        
        // Recover signer address
        address recovered = ecrecover(ethSignedHash, v, r, s);
        if (recovered == address(0) || recovered != signer) revert InvalidSignature();
    }
    
    /**
     * @notice Hash event data using SHA-256 and JSON serialization (aligned with SVM)
     * @dev This must match the SVM implementation: SHA-256(JSON.stringify(eventData))
     */
    function _hashEventData(StakeEventData memory eventData) internal pure returns (bytes32) {
        // Serialize event data to JSON-like format to match SVM
        // Format: {"sourceContract":"...","targetContract":"...","chainId":"...","blockHeight":"...","amount":"...","receiverAddress":"...","nonce":"..."}
        bytes memory json = abi.encodePacked(
            '{"sourceContract":"', _bytes32ToString(eventData.sourceContract),
            '","targetContract":"', _bytes32ToString(eventData.targetContract),
            '","chainId":"', _uint64ToString(eventData.sourceChainId),
            '","blockHeight":"', _uint64ToString(eventData.blockHeight),
            '","amount":"', _uint64ToString(eventData.amount),
            '","receiverAddress":"', eventData.receiverAddress,
            '","nonce":"', _uint64ToString(eventData.nonce),
            '"}'
        );
        
        return sha256(json);
    }
    
    /**
     * @notice Convert bytes32 to hex string (lowercase, no 0x prefix)
     * @dev Converts full 32 bytes to 64 character hex string to support cross-chain addresses
     */
    function _bytes32ToString(bytes32 data) internal pure returns (string memory) {
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
     * @notice Convert uint64 to string
     */
    function _uint64ToString(uint64 value) internal pure returns (string memory) {
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
    
    /**
     * @notice Parse address from string (hex format)
     */
    function _parseAddress(string memory addressStr) internal pure returns (address) {
        bytes memory strBytes = bytes(addressStr);
        require(strBytes.length == 40 || strBytes.length == 42, "Invalid address length");
        
        uint256 start = 0;
        if (strBytes.length == 42) {
            require(strBytes[0] == '0' && (strBytes[1] == 'x' || strBytes[1] == 'X'), "Invalid hex prefix");
            start = 2;
        }
        
        uint160 result = 0;
        for (uint i = start; i < strBytes.length; i++) {
            uint8 digit = uint8(strBytes[i]);
            uint8 value;
            
            if (digit >= 48 && digit <= 57) {
                value = digit - 48;
            } else if (digit >= 65 && digit <= 70) {
                value = digit - 55;
            } else if (digit >= 97 && digit <= 102) {
                value = digit - 87;
            } else {
                revert("Invalid hex character");
            }
            
            result = result * 16 + value;
        }
        
        return address(result);
    }
}

