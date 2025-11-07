// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/**
 * @title CoreContract
 * @notice Core bridge contract for cross-chain messaging
 * @dev Implements Wormhole-style VAA verification and message publishing
 */
contract CoreContract {
    // ===== State Variables =====
    
    /// @notice Current Guardian Set index
    uint32 public guardianSetIndex;
    
    /// @notice Guardian Set storage
    struct GuardianSet {
        address[] keys;
        uint32 expirationTime;
    }
    mapping(uint32 => GuardianSet) public guardianSets;
    
    /// @notice Consumed VAAs (防重放攻击)
    mapping(bytes32 => bool) public consumedVAAs;
    
    /// @notice Message sequence counter per sender
    mapping(address => uint64) public sequences;
    
    /// @notice Chain ID
    uint16 public immutable chainId;
    
    /// @notice Message fee
    uint256 public messageFee;
    
    /// @notice Contract paused flag
    bool public paused;
    
    /// @notice Contract owner (for initialization)
    address public owner;
    
    // ===== Events =====
    
    event LogMessagePublished(
        address indexed sender,
        uint64 sequence,
        uint32 nonce,
        bytes payload,
        uint8 consistencyLevel
    );
    
    event GuardianSetAdded(uint32 indexed index);
    
    event GuardianSetUpdated(
        uint32 indexed oldIndex,
        uint32 indexed newIndex
    );
    
    event ContractPausedStateChanged(bool paused);
    
    // ===== Errors =====
    
    error InsufficientFee();
    error BridgePaused();
    error InvalidGuardianSetIndex();
    error VAAAlreadyConsumed();
    error InsufficientSignatures();
    error InvalidSignature();
    error InvalidGuardianIndex();
    error OnlyOwner();
    
    // ===== Constructor =====
    
    constructor(
        uint16 _chainId,
        address[] memory initialGuardians,
        uint256 _messageFee
    ) {
        require(initialGuardians.length > 0, "No guardians provided");
        
        chainId = _chainId;
        owner = msg.sender;
        messageFee = _messageFee;
        
        // Initialize first Guardian Set
        GuardianSet storage guardianSet = guardianSets[0];
        guardianSet.keys = initialGuardians;
        guardianSet.expirationTime = 0; // Active indefinitely
        
        guardianSetIndex = 0;
        
        emit GuardianSetAdded(0);
    }
    
    // ===== Modifiers =====
    
    modifier whenNotPaused() {
        if (paused) revert BridgePaused();
        _;
    }
    
    modifier onlyOwner() {
        if (msg.sender != owner) revert OnlyOwner();
        _;
    }
    
    // ===== Public Functions =====
    
    /**
     * @notice Publish a cross-chain message
     * @param nonce Unique nonce for replay protection
     * @param payload Message payload
     * @param consistencyLevel Confirmation depth (e.g., 200 for finalized)
     * @return sequence Message sequence number
     */
    function publishMessage(
        uint32 nonce,
        bytes memory payload,
        uint8 consistencyLevel
    ) external payable whenNotPaused returns (uint64 sequence) {
        if (msg.value < messageFee) revert InsufficientFee();
        
        // Get and increment sequence
        sequence = sequences[msg.sender]++;
        
        // Emit event for Guardians to observe
        emit LogMessagePublished(
            msg.sender,
            sequence,
            nonce,
            payload,
            consistencyLevel
        );
        
        return sequence;
    }
    
    /**
     * @notice Get current Guardian Set
     * @return Current GuardianSet
     */
    function getCurrentGuardianSet() 
        external 
        view 
        returns (GuardianSet memory) 
    {
        return guardianSets[guardianSetIndex];
    }
    
    /**
     * @notice Get Guardian Set by index
     * @param index Guardian Set index
     * @return GuardianSet at the given index
     */
    function getGuardianSet(uint32 index) 
        external 
        view 
        returns (GuardianSet memory) 
    {
        return guardianSets[index];
    }
    
    /**
     * @notice Get guardian count in current set
     * @return Number of guardians
     */
    function getGuardianSetSize() external view returns (uint256) {
        return guardianSets[guardianSetIndex].keys.length;
    }
    
    /**
     * @notice Calculate required quorum (2/3 + 1)
     * @return Required number of signatures
     */
    function quorum() public view returns (uint8) {
        uint256 guardianCount = guardianSets[guardianSetIndex].keys.length;
        return uint8((guardianCount * 2) / 3 + 1);
    }
    
    /**
     * @notice Parse and verify a VAA
     * @param encodedVAA Encoded VAA bytes
     * @return isValid Whether the VAA is valid
     * @return vaaHash Hash of the VAA body
     */
    function parseAndVerifyVAA(bytes calldata encodedVAA) 
        external 
        returns (bool isValid, bytes32 vaaHash) 
    {
        // Parse VAA header and get body offset
        (uint32 guardianSetIdx, uint8 signaturesLen, uint256 bodyOffset) = 
            _parseVAAHeader(encodedVAA);
        
        // Calculate VAA body hash
        vaaHash = _calculateVAAHash(encodedVAA, bodyOffset);
        
        // Check if already consumed
        if (consumedVAAs[vaaHash]) revert VAAAlreadyConsumed();
        
        // Verify signatures
        _verifyVAASignatures(encodedVAA, guardianSetIdx, signaturesLen, vaaHash);
        
        // Mark as consumed
        consumedVAAs[vaaHash] = true;
        
        return (true, vaaHash);
    }
    
    /**
     * @dev Parse VAA header
     */
    function _parseVAAHeader(bytes calldata encodedVAA) 
        private 
        pure 
        returns (uint32 guardianSetIdx, uint8 signaturesLen, uint256 bodyOffset) 
    {
        uint256 index = 0;
        
        // Version
        uint8 version = uint8(encodedVAA[index]);
        index += 1;
        require(version == 1, "Invalid VAA version");
        
        // Guardian set index
        guardianSetIdx = uint32(uint8(encodedVAA[index])) << 24;
        guardianSetIdx |= uint32(uint8(encodedVAA[index + 1])) << 16;
        guardianSetIdx |= uint32(uint8(encodedVAA[index + 2])) << 8;
        guardianSetIdx |= uint32(uint8(encodedVAA[index + 3]));
        index += 4;
        
        // Signatures count
        signaturesLen = uint8(encodedVAA[index]);
        index += 1;
        
        // Body offset = header (6) + signatures (66 * count)
        bodyOffset = 6 + (66 * signaturesLen);
    }
    
    /**
     * @dev Calculate VAA hash (double keccak256)
     */
    function _calculateVAAHash(bytes calldata encodedVAA, uint256 bodyOffset) 
        private 
        pure 
        returns (bytes32) 
    {
        bytes32 bodyHash = keccak256(encodedVAA[bodyOffset:]);
        return keccak256(abi.encodePacked(bodyHash));
    }
    
    /**
     * @dev Verify VAA signatures
     */
    function _verifyVAASignatures(
        bytes calldata encodedVAA,
        uint32 guardianSetIdx,
        uint8 signaturesLen,
        bytes32 vaaHash
    ) private view {
        // Get guardian set
        GuardianSet storage guardianSet = guardianSets[guardianSetIdx];
        if (guardianSet.keys.length == 0) revert InvalidGuardianSetIndex();
        
        // Check quorum
        uint8 requiredSigs = uint8((guardianSet.keys.length * 2) / 3 + 1);
        if (signaturesLen < requiredSigs) revert InsufficientSignatures();
        
        // Verify each signature
        uint256 index = 6; // Start after header
        int16 lastIndex = -1; // Allow first guardian to be index 0
        
        for (uint256 i = 0; i < signaturesLen; i++) {
            // Parse signature
            uint8 guardianIndex = uint8(encodedVAA[index]);
            index += 1;
            
            bytes32 r = bytes32(encodedVAA[index:index + 32]);
            index += 32;
            
            bytes32 s = bytes32(encodedVAA[index:index + 32]);
            index += 32;
            
            uint8 v = uint8(encodedVAA[index]);
            index += 1;
            
            // Verify guardian index is ascending (prevents duplicates)
            require(int16(uint16(guardianIndex)) > lastIndex, "Invalid guardian order");
            require(guardianIndex < guardianSet.keys.length, "Guardian index out of bounds");
            lastIndex = int16(uint16(guardianIndex));
            
            // Verify signature
            address signer = ecrecover(vaaHash, v, r, s);
            if (signer != guardianSet.keys[guardianIndex]) revert InvalidSignature();
        }
    }
    
    // ===== Admin Functions =====
    
    /**
     * @notice Pause the contract
     */
    function pause() external onlyOwner {
        paused = true;
        emit ContractPausedStateChanged(true);
    }
    
    /**
     * @notice Unpause the contract
     */
    function unpause() external onlyOwner {
        paused = false;
        emit ContractPausedStateChanged(false);
    }
    
    /**
     * @notice Update message fee
     * @param newFee New fee amount
     */
    function updateMessageFee(uint256 newFee) external onlyOwner {
        messageFee = newFee;
    }
    
    /**
     * @notice Withdraw collected fees
     * @param recipient Recipient address
     */
    function withdrawFees(address payable recipient) external onlyOwner {
        uint256 balance = address(this).balance;
        recipient.transfer(balance);
    }
}

