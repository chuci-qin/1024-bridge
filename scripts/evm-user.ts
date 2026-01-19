#!/usr/bin/env ts-node

/**
 * EVM User Operations Script
 * 
 * 用户可用功能：
 * 1. stake - 质押 USDC 到跨链桥
 */

import { ethers } from 'ethers';
import * as dotenv from 'dotenv';
import * as path from 'path';

// 加载环境变量
dotenv.config({ path: path.resolve(__dirname, '../.env.invoke') });

// ============ ABI 定义 ============

const BRIDGE_ABI = [
  'function stake(uint256 amount, string memory receiverAddress) external returns (uint64)',
  'function senderState() external view returns (address vault, address admin, address usdcContract, uint64 nonce, address targetContract, uint64 sourceChainId, uint64 targetChainId)',
];

const ERC20_ABI = [
  'function approve(address spender, uint256 amount) external returns (bool)',
  'function allowance(address owner, address spender) external view returns (uint256)',
  'function balanceOf(address account) external view returns (uint256)',
  'function decimals() external view returns (uint8)',
];

// ============ 配置 ============

interface Config {
  rpcUrl: string;
  contractAddress: string;
  userPrivateKey: string;
  usdcContract: string;
  testAmount: string;
  testReceiver: string;
}

function loadConfig(): Config {
  return {
    rpcUrl: process.env.EVM_RPC_URL || 'https://sepolia-rollup.arbitrum.io/rpc',
    contractAddress: process.env.EVM_CONTRACT_ADDRESS || '',
    userPrivateKey: process.env.USER_EVM_PRIVATE_KEY || '',
    usdcContract: process.env.USDC_EVM_CONTRACT || '',
    testAmount: process.env.TEST_STAKE_AMOUNT || '1000000',
    testReceiver: process.env.TEST_RECEIVER_ADDRESS_SVM || '',
  };
}

// ============ 辅助函数 ============

function printHeader(title: string) {
  console.log('\n============================================');
  console.log(title);
  console.log('============================================\n');
}

function printSuccess(message: string) {
  console.log(`✓ ${message}`);
}

function printError(message: string) {
  console.error(`✗ ${message}`);
}

// ============ 用户操作：质押 ============

async function stake(amount?: string, receiver?: string) {
  printHeader('用户质押 (User Stake)');

  const config = loadConfig();
  const stakeAmount = amount || config.testAmount;
  const receiverAddress = receiver || config.testReceiver;

  // 创建 provider 和 wallet
  const provider = new ethers.JsonRpcProvider(config.rpcUrl);
  const wallet = new ethers.Wallet(config.userPrivateKey, provider);

  // 创建合约实例
  const bridge = new ethers.Contract(config.contractAddress, BRIDGE_ABI, wallet);
  const usdc = new ethers.Contract(config.usdcContract, ERC20_ABI, wallet);

  console.log('配置信息:');
  console.log(`  RPC: ${config.rpcUrl}`);
  console.log(`  Bridge Contract: ${config.contractAddress}`);
  console.log(`  USDC Contract: ${config.usdcContract}`);
  console.log(`  User: ${wallet.address}`);
  console.log(`  Amount: ${stakeAmount}`);
  console.log(`  Receiver: ${receiverAddress}`);
  console.log('');

  try {
    // 1. 检查 USDC 余额
    console.log('检查 USDC 余额...');
    const balance = await usdc.balanceOf(wallet.address);
    console.log(`  USDC Balance: ${ethers.formatUnits(balance, 6)} USDC`);

    if (balance < BigInt(stakeAmount)) {
      throw new Error('Insufficient USDC balance');
    }

    // 2. 检查并批准 USDC
    console.log('检查 USDC 授权...');
    const currentAllowance = await usdc.allowance(wallet.address, config.contractAddress);
    console.log(`  Current Allowance: ${ethers.formatUnits(currentAllowance, 6)} USDC`);

    if (currentAllowance < BigInt(stakeAmount)) {
      console.log('授权 USDC 给合约...');
      const approveTx = await usdc.approve(config.contractAddress, stakeAmount);
      console.log(`  Approve TX: ${approveTx.hash}`);
      
      const approveReceipt = await approveTx.wait();
      printSuccess('USDC 授权成功！');
      console.log(`  Gas Used: ${approveReceipt?.gasUsed.toString()}`);
      console.log('');
    } else {
      printSuccess('USDC 授权已足够');
      console.log('');
    }

    // 3. 执行 stake
    console.log('执行质押交易...');
    const stakeTx = await bridge.stake(stakeAmount, receiverAddress);
    console.log(`  Stake TX: ${stakeTx.hash}`);
    console.log(`  Waiting for confirmation...`);

    const stakeReceipt = await stakeTx.wait();
    printSuccess('质押成功！');
    console.log(`  Block Number: ${stakeReceipt?.blockNumber}`);
    console.log(`  Gas Used: ${stakeReceipt?.gasUsed.toString()}`);
    
    // 解析事件获取 nonce
    if (stakeReceipt?.logs) {
      console.log(`  Total Logs: ${stakeReceipt.logs.length}`);
      // TODO: 解析 StakeEvent 获取 nonce
    }

    console.log('');
    console.log(`Explorer: https://sepolia.arbiscan.io/tx/${stakeTx.hash}`);

  } catch (error: any) {
    printError(`质押失败: ${error.message}`);
    if (error.reason) {
      console.error(`  Reason: ${error.reason}`);
    }
    throw error;
  }
}

// ============ 查询操作 ============

async function queryBalance() {
  printHeader('查询用户余额');

  const config = loadConfig();
  const provider = new ethers.JsonRpcProvider(config.rpcUrl);
  const wallet = new ethers.Wallet(config.userPrivateKey, provider);
  const usdc = new ethers.Contract(config.usdcContract, ERC20_ABI, provider);

  try {
    // 查询 ETH 余额
    const ethBalance = await provider.getBalance(wallet.address);
    console.log(`ETH Balance: ${ethers.formatEther(ethBalance)} ETH`);

    // 查询 USDC 余额
    const usdcBalance = await usdc.balanceOf(wallet.address);
    console.log(`USDC Balance: ${ethers.formatUnits(usdcBalance, 6)} USDC`);

    // 查询授权额度
    const allowance = await usdc.allowance(wallet.address, config.contractAddress);
    console.log(`USDC Allowance: ${ethers.formatUnits(allowance, 6)} USDC`);

  } catch (error: any) {
    printError(`查询失败: ${error.message}`);
  }
}

async function queryContractState() {
  printHeader('查询合约状态');

  const config = loadConfig();
  const provider = new ethers.JsonRpcProvider(config.rpcUrl);
  const bridge = new ethers.Contract(config.contractAddress, BRIDGE_ABI, provider);

  try {
    const senderState = await bridge.senderState();
    console.log('Sender State:');
    console.log(`  Vault: ${senderState[0]}`);
    console.log(`  Admin: ${senderState[1]}`);
    console.log(`  USDC Contract: ${senderState[2]}`);
    console.log(`  Nonce: ${senderState[3].toString()}`);
    console.log(`  Target Contract: ${senderState[4]}`);
    console.log(`  Source Chain ID: ${senderState[5].toString()}`);
    console.log(`  Target Chain ID: ${senderState[6].toString()}`);

  } catch (error: any) {
    printError(`查询失败: ${error.message}`);
  }
}

// ============ 主程序 ============

async function main() {
  const args = process.argv.slice(2);
  const command = args[0];

  if (!command) {
    console.log('Usage: ts-node evm-user.ts <command> [options]');
    console.log('');
    console.log('Commands:');
    console.log('  stake [amount] [receiver]  - 质押 USDC');
    console.log('  balance                    - 查询余额');
    console.log('  state                      - 查询合约状态');
    console.log('');
    console.log('Examples:');
    console.log('  ts-node evm-user.ts stake');
    console.log('  ts-node evm-user.ts stake 1000000 receiver_pubkey');
    console.log('  ts-node evm-user.ts balance');
    console.log('  ts-node evm-user.ts state');
    return;
  }

  try {
    switch (command) {
      case 'stake':
        const amount = args[1];
        const receiver = args[2];
        await stake(amount, receiver);
        break;

      case 'balance':
        await queryBalance();
        break;

      case 'state':
        await queryContractState();
        break;

      default:
        printError(`Unknown command: ${command}`);
        process.exit(1);
    }

    printSuccess('操作完成！');
  } catch (error: any) {
    printError(`操作失败: ${error?.message || error}`);
    if (error?.stack) {
      console.error(error.stack);
    }
    process.exit(1);
  }
}

// 运行主程序
if (require.main === module) {
  main();
}

export { stake, queryBalance, queryContractState };
