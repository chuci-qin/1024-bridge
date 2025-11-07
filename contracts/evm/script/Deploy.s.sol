// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/CoreContract.sol";

contract DeployScript is Script {
    function run() external {
        // Use provided private key or default Anvil key
        uint256 deployerPrivateKey;
        try vm.envUint("PRIVATE_KEY") returns (uint256 key) {
            deployerPrivateKey = key;
        } catch {
            // Default Anvil account #0
            deployerPrivateKey = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
        }
        
        vm.startBroadcast(deployerPrivateKey);
        
        // For local testing, create 19 mock guardian addresses
        address[] memory guardians = new address[](19);
        for (uint i = 0; i < 19; i++) {
            guardians[i] = address(uint160(0x1000 + i));
        }
        
        // Deploy CoreContract
        uint16 chainId = 1; // EVM chain
        uint256 messageFee = 0.001 ether;
        
        CoreContract core = new CoreContract(chainId, guardians, messageFee);
        
        console.log("CoreContract deployed at:", address(core));
        console.log("Chain ID:", chainId);
        console.log("Guardian Set Size:", core.getGuardianSetSize());
        console.log("Quorum:", core.quorum());
        
        vm.stopBroadcast();
    }
}

