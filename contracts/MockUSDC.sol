// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/**
 * @title MockUSDC
 * @notice Mock USDC合约（仅用于测试）
 */
contract MockUSDC is ERC20 {
    constructor() ERC20("Mock USDC", "USDC") {
        // Mint 1M USDC给部署者（测试用）
        _mint(msg.sender, 1_000_000 * 1e6);
    }
    
    function decimals() public pure override returns (uint8) {
        return 6;  // USDC是6位小数
    }
    
    // 测试用：任何人都可以mint
    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

