// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/CoreContract.sol";

contract VAABasicTest is Test {
    CoreContract public core;
    
    function setUp() public {
        // Use simple guardian addresses
        address[] memory guardians = new address[](19);
        for (uint i = 0; i < 19; i++) {
            guardians[i] = address(uint160(0x1000 + i));
        }
        
        core = new CoreContract(1, guardians, 0);
    }
    
    function testBasicFunctions() public {
        assertEq(core.guardianSetIndex(), 0);
        assertEq(core.getGuardianSetSize(), 19);
        assertEq(core.quorum(), 13);
    }
    
    function testConsumedVAAsMapping() public {
        bytes32 testHash = keccak256("test");
        assertFalse(core.consumedVAAs(testHash));
    }
    
    function testPublishMessage() public {
        uint64 seq = core.publishMessage(0, hex"74657374", 200);
        assertEq(seq, 0);
    }
}

