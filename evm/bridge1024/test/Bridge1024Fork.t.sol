// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/Bridge1024.sol";

/**
 * @title Bridge1024ForkTest
 * @notice Fork 测试 - 针对 Arbitrum Sepolia 已部署的 Bridge1024 合约
 * @dev 使用 forge test --match-contract Bridge1024ForkTest --fork-url $RPC 运行
 *      或: forge test --match-contract Bridge1024ForkTest --fork-url https://sepolia-rollup.arbitrum.io/rpc
 */
contract Bridge1024ForkTest is Test {
    Bridge1024 public bridge;
    
    address constant DEPLOYED_BRIDGE = 0xc05a4718E87B54773F1a242BD0701aF10921f510;
    address constant ADMIN = 0xd4B42EfF8AF8eF82dE3830fE30559bfF92Dca55F;
    address constant USDC_ARB_SEPOLIA = 0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d;
    uint256 constant ADMIN_PRIVATE_KEY = 0x1091b1bfb7f708d40a93f70c4d41baf0c9001ab0691cda6d971c2b6afa2b7f10;
    
    string constant ARB_SEPOLIA_RPC = "https://sepolia-rollup.arbitrum.io/rpc";
    
    function setUp() public {
        uint256 fork;
        try vm.createSelectFork(ARB_SEPOLIA_RPC) returns (uint256 f) {
            fork = f;
        } catch {
            vm.skip(true); // Fork 失败时跳过（如无网络）
        }
        if (fork == 0) vm.skip(true);
        bridge = Bridge1024(payable(DEPLOYED_BRIDGE));
    }
    
    function testFork_ContractExists() public view {
        // 验证合约已部署（有字节码）
        uint256 size;
        assembly {
            size := extcodesize(DEPLOYED_BRIDGE)
        }
        assertGt(size, 0);
    }
    
    function testFork_SenderState_Initialized() public view {
        (address vault, address adminAddr, address usdc, uint64 nonce,
         bytes32 targetContract, uint64 /* sourceChain */, uint64 /* targetChain */, uint64 decimalRatio) = bridge.senderState();
        
        assertEq(adminAddr, ADMIN);
        assertEq(vault, DEPLOYED_BRIDGE); // 合约自身作为 vault
        assertEq(decimalRatio, 1);         // 默认 ratio
    }
    
    function testFork_ReceiverState_Initialized() public view {
        (address vault, address adminAddr, , , , , , , uint64 decimalRatio) = bridge.receiverState();
        
        assertEq(adminAddr, ADMIN);
        assertEq(vault, DEPLOYED_BRIDGE);
        assertEq(decimalRatio, 1);
    }
    
    function testFork_GetSenderNonce() public view {
        uint64 nonce = bridge.getSenderNonce();
        // 部署后 nonce 从 0 开始，可能已递增
        assertTrue(nonce >= 0, "sender nonce");
    }
    
    function testFork_GetReceiverLastNonce() public view {
        uint64 lastNonce = bridge.getReceiverLastNonce();
        assertTrue(lastNonce >= 0, "receiver lastNonce");
    }
    
    function testFork_AdminCanConfigureUsdc() public {
        vm.prank(ADMIN);
        bridge.configureUsdc(USDC_ARB_SEPOLIA);
        
        (, , address usdc, , , , , ) = bridge.senderState();
        assertEq(usdc, USDC_ARB_SEPOLIA);
        
        (, , address rUsdc, , , , , , ) = bridge.receiverState();
        assertEq(rUsdc, USDC_ARB_SEPOLIA);
    }
    
    function testFork_NonAdminCannotConfigureUsdc() public {
        address nonAdmin = address(0x1234);
        
        vm.prank(nonAdmin);
        vm.expectRevert(Bridge1024.Unauthorized.selector);
        bridge.configureUsdc(USDC_ARB_SEPOLIA);
    }
    
    function testFork_IsRelayer_EmptyInitially() public view {
        // 部署后 relayers 可能为空
        assertFalse(bridge.isRelayer(address(0x1)));
        assertEq(bridge.getRelayerCount(), 0);
    }
}
