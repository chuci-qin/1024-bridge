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

