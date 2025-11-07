// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/CoreContract.sol";

contract CoreContractTest is Test {
    CoreContract public core;
    
    address[] public guardians;
    uint16 constant CHAIN_ID = 1;
    uint256 constant MESSAGE_FEE = 0.001 ether;
    
    address public user1;
    address public user2;
    
    function setUp() public {
        // Setup test accounts
        user1 = address(0x1);
        user2 = address(0x2);
        
        // Create 19 guardian addresses
        for (uint i = 0; i < 19; i++) {
            guardians.push(address(uint160(0x1000 + i)));
        }
        
        // Deploy contract
        core = new CoreContract(CHAIN_ID, guardians, MESSAGE_FEE);
    }
    
    function testInitialization() public {
        assertEq(core.chainId(), CHAIN_ID);
        assertEq(core.guardianSetIndex(), 0);
        assertEq(core.getGuardianSetSize(), 19);
        assertEq(core.messageFee(), MESSAGE_FEE);
        assertEq(core.paused(), false);
    }
    
    function testQuorumCalculation() public {
        // 19 guardians -> 13 required
        assertEq(core.quorum(), 13);
    }
    
    function testPublishMessage() public {
        vm.deal(user1, 1 ether);
        
        bytes memory payload = abi.encode("Hello, Solana!");
        uint32 nonce = 12345;
        uint8 consistencyLevel = 200;
        
        vm.prank(user1);
        uint64 sequence = core.publishMessage{value: MESSAGE_FEE}(
            nonce,
            payload,
            consistencyLevel
        );
        
        assertEq(sequence, 0);
        assertEq(core.sequences(user1), 1);
    }
    
    function testPublishMessageInsufficientFee() public {
        vm.deal(user1, 1 ether);
        
        bytes memory payload = abi.encode("Test");
        
        vm.prank(user1);
        vm.expectRevert(CoreContract.InsufficientFee.selector);
        core.publishMessage{value: MESSAGE_FEE - 1}(
            0,
            payload,
            200
        );
    }
    
    function testMultipleMessages() public {
        vm.deal(user1, 1 ether);
        
        vm.startPrank(user1);
        
        uint64 seq1 = core.publishMessage{value: MESSAGE_FEE}(
            0,
            abi.encode("Message 1"),
            200
        );
        
        uint64 seq2 = core.publishMessage{value: MESSAGE_FEE}(
            0,
            abi.encode("Message 2"),
            200
        );
        
        uint64 seq3 = core.publishMessage{value: MESSAGE_FEE}(
            0,
            abi.encode("Message 3"),
            200
        );
        
        vm.stopPrank();
        
        assertEq(seq1, 0);
        assertEq(seq2, 1);
        assertEq(seq3, 2);
        assertEq(core.sequences(user1), 3);
    }
    
    function testPause() public {
        core.pause();
        assertEq(core.paused(), true);
        
        vm.deal(user1, 1 ether);
        vm.prank(user1);
        vm.expectRevert(CoreContract.BridgePaused.selector);
        core.publishMessage{value: MESSAGE_FEE}(
            0,
            abi.encode("Test"),
            200
        );
    }
    
    function testUnpause() public {
        core.pause();
        core.unpause();
        assertEq(core.paused(), false);
        
        // Should work now
        vm.deal(user1, 1 ether);
        vm.prank(user1);
        core.publishMessage{value: MESSAGE_FEE}(
            0,
            abi.encode("Test"),
            200
        );
    }
    
    function testOnlyOwnerCanPause() public {
        vm.prank(user1);
        vm.expectRevert(CoreContract.OnlyOwner.selector);
        core.pause();
    }
    
    function testUpdateMessageFee() public {
        uint256 newFee = 0.002 ether;
        core.updateMessageFee(newFee);
        assertEq(core.messageFee(), newFee);
    }
    
    function testWithdrawFees() public {
        // Send some messages to accumulate fees
        vm.deal(user1, 1 ether);
        vm.startPrank(user1);
        
        for (uint i = 0; i < 5; i++) {
            core.publishMessage{value: MESSAGE_FEE}(
                uint32(i),
                abi.encode("Test"),
                200
            );
        }
        
        vm.stopPrank();
        
        // Withdraw fees
        address payable recipient = payable(user2);
        uint256 balanceBefore = recipient.balance;
        
        core.withdrawFees(recipient);
        
        uint256 balanceAfter = recipient.balance;
        assertEq(balanceAfter - balanceBefore, MESSAGE_FEE * 5);
    }
    
    function testGetGuardianSet() public {
        CoreContract.GuardianSet memory gs = core.getCurrentGuardianSet();
        assertEq(gs.keys.length, 19);
        assertEq(gs.expirationTime, 0);
    }
}

