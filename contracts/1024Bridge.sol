// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/security/Pausable.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import "@openzeppelin/contracts/access/AccessControl.sol";

/**
 * @title 1024Bridge
 * @notice 官方桥：Arbitrum ↔ 1024Chain（Solana）
 * @dev 两层桥接架构的第二层
 */
contract 1024Bridge is Pausable, ReentrancyGuard, AccessControl {
    // ========================================
    // 角色定义
    // ========================================
    
    bytes32 public constant RELAYER_ROLE = keccak256("RELAYER_ROLE");
    bytes32 public constant GUARDIAN_ROLE = keccak256("GUARDIAN_ROLE");
    
    // ========================================
    // 状态变量
    // ========================================
    
    IERC20 public immutable USDC;
    uint256 public nonce;
    
    // 用户充值记录（用于提现验证）
    mapping(address => uint256) public userDeposits;
    uint256 public totalUserDeposits;
    
    // 已处理的提现（防止重放攻击）
    mapping(bytes32 => bool) public processedWithdrawals;
    
    // Phase 3扩展：流动性挖矿（预留）
    mapping(address => uint256) public lpStakes;
    uint256 public totalLPStaked;
    uint256 public accumulatedFees;
    
    // 限额配置
    uint256 public maxSingleAmount = 100_000 * 1e6;  // 100K USDC
    uint256 public maxHourlyVolume = 1_000_000 * 1e6;  // 1M USDC/小时
    uint256 public hourlyVolume;
    uint256 public lastHourTimestamp;
    
    // 手续费（Phase 2收集，Phase 3分配）
    uint256 public constant BRIDGE_FEE_BPS = 10;  // 0.1%
    
    // ========================================
    // 事件
    // ========================================
    
    event USDCLocked(
        address indexed user,
        string solanaAddress,
        uint256 amount,
        uint256 fee,
        uint256 indexed nonce
    );
    
    event USDCUnlocked(
        address indexed user,
        uint256 amount,
        bytes32 indexed burnTxHash,
        uint256 indexed nonce
    );
    
    event LiquidityStaked(
        address indexed lp,
        uint256 amount
    );
    
    event LiquidityUnstaked(
        address indexed lp,
        uint256 amount
    );
    
    // ========================================
    // 构造函数
    // ========================================
    
    constructor(address _usdc) {
        require(_usdc != address(0), "Invalid USDC address");
        
        USDC = IERC20(_usdc);
        
        // 设置角色
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(GUARDIAN_ROLE, msg.sender);
        
        lastHourTimestamp = block.timestamp;
    }
    
    // ========================================
    // 充值功能（Arbitrum → Solana）
    // ========================================
    
    /**
     * @notice 锁定USDC并桥接到1024Chain（Solana）
     * @param amount USDC金额（6位小数）
     * @param solanaAddress 目标Solana地址（Base58格式）
     */
    function lockUSDC(
        uint256 amount,
        string calldata solanaAddress
    ) external nonReentrant whenNotPaused {
        require(amount > 0, "Amount must be > 0");
        require(bytes(solanaAddress).length >= 32, "Invalid Solana address");
        require(amount <= maxSingleAmount, "Exceeds single tx limit");
        
        // 检查每小时限额
        _checkHourlyLimit(amount);
        
        // 计算手续费
        uint256 fee = amount * BRIDGE_FEE_BPS / 10000;
        uint256 netAmount = amount - fee;
        
        // 转入USDC到合约
        require(
            USDC.transferFrom(msg.sender, address(this), amount),
            "Transfer failed"
        );
        
        // 更新状态
        userDeposits[msg.sender] += netAmount;
        totalUserDeposits += netAmount;
        accumulatedFees += fee;
        
        // 发出Lock事件（Relayer监听此事件）
        emit USDCLocked(msg.sender, solanaAddress, netAmount, fee, nonce);
        nonce++;
    }
    
    // ========================================
    // 提现功能（Solana → Arbitrum）
    // ========================================
    
    /**
     * @notice 解锁USDC到Arbitrum地址（Relayer调用）
     * @param user 用户Arbitrum地址
     * @param amount USDC金额
     * @param burnTxHash Solana上的burn交易哈希
     */
    function unlockUSDC(
        address user,
        uint256 amount,
        bytes32 burnTxHash
    ) external onlyRole(RELAYER_ROLE) nonReentrant whenNotPaused {
        require(amount > 0, "Amount must be > 0");
        require(user != address(0), "Invalid user address");
        require(!processedWithdrawals[burnTxHash], "Already processed");
        
        // 检查合约流动性充足
        require(getTotalLiquidity() >= amount, "Insufficient liquidity");
        
        // 标记已处理（防止重放）
        processedWithdrawals[burnTxHash] = true;
        
        // 转出USDC
        require(USDC.transfer(user, amount), "Transfer failed");
        
        emit USDCUnlocked(user, amount, burnTxHash, nonce);
        nonce++;
    }
    
    // ========================================
    // Phase 3扩展：流动性挖矿（预留）
    // ========================================
    
    /**
     * @notice LP质押USDC（Phase 3实现）
     * @param amount 质押金额
     */
    function stakeLiquidity(uint256 amount) external {
        revert("Phase 3 feature - Not implemented yet");
        
        // Phase 3实现：
        // USDC.transferFrom(msg.sender, address(this), amount);
        // lpStakes[msg.sender] += amount;
        // totalLPStaked += amount;
        // emit LiquidityStaked(msg.sender, amount);
    }
    
    /**
     * @notice LP取消质押（Phase 3实现）
     * @param amount 取消质押金额
     */
    function unstakeLiquidity(uint256 amount) external {
        revert("Phase 3 feature - Not implemented yet");
        
        // Phase 3实现：
        // require(lpStakes[msg.sender] >= amount, "Insufficient stake");
        // require(_canUnstake(amount), "Liquidity needed");
        // lpStakes[msg.sender] -= amount;
        // totalLPStaked -= amount;
        // USDC.transfer(msg.sender, amount);
        // emit LiquidityUnstaked(msg.sender, amount);
    }
    
    /**
     * @notice 计算LP奖励（Phase 3实现）
     */
    function calculateLPRewards(address lp) public view returns (uint256) {
        return 0;  // Phase 2返回0，Phase 3实现
    }
    
    // ========================================
    // 查询函数
    // ========================================
    
    /**
     * @notice 获取总流动性
     * @return 总流动性（用户充值 + LP质押）
     */
    function getTotalLiquidity() public view returns (uint256) {
        // Phase 2: 只有用户资金
        return USDC.balanceOf(address(this));
        
        // Phase 3: 用户资金 + LP质押
        // return totalUserDeposits + totalLPStaked;
    }
    
    /**
     * @notice 获取用户充值总额
     */
    function getUserDeposit(address user) external view returns (uint256) {
        return userDeposits[user];
    }
    
    // ========================================
    // 管理功能
    // ========================================
    
    /**
     * @notice 紧急暂停（3-of-5 MultiSig）
     */
    function pause() external onlyRole(GUARDIAN_ROLE) {
        _pause();
    }
    
    /**
     * @notice 恢复运行
     */
    function unpause() external onlyRole(GUARDIAN_ROLE) {
        _unpause();
    }
    
    /**
     * @notice 更新限额配置
     */
    function updateLimits(
        uint256 _maxSingleAmount,
        uint256 _maxHourlyVolume
    ) external onlyRole(DEFAULT_ADMIN_ROLE) {
        maxSingleAmount = _maxSingleAmount;
        maxHourlyVolume = _maxHourlyVolume;
    }
    
    /**
     * @notice 添加Relayer
     */
    function addRelayer(address relayer) external onlyRole(DEFAULT_ADMIN_ROLE) {
        grantRole(RELAYER_ROLE, relayer);
    }
    
    /**
     * @notice 移除Relayer
     */
    function removeRelayer(address relayer) external onlyRole(DEFAULT_ADMIN_ROLE) {
        revokeRole(RELAYER_ROLE, relayer);
    }
    
    // ========================================
    // 内部函数
    // ========================================
    
    /**
     * @dev 检查每小时限额
     */
    function _checkHourlyLimit(uint256 amount) internal {
        // 重置计数器（每小时）
        if (block.timestamp >= lastHourTimestamp + 1 hours) {
            hourlyVolume = 0;
            lastHourTimestamp = block.timestamp;
        }
        
        require(hourlyVolume + amount <= maxHourlyVolume, "Hourly limit exceeded");
        hourlyVolume += amount;
    }
    
    /**
     * @dev 检查是否可以取消LP质押（Phase 3）
     */
    function _canUnstake(uint256 amount) internal view returns (bool) {
        // 确保提现后仍有足够流动性
        uint256 afterUnstake = getTotalLiquidity() - amount;
        uint256 minRequired = totalUserDeposits * 20 / 100;  // 至少保留20%
        return afterUnstake >= minRequired;
    }
}

