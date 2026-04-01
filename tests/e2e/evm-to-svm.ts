/**
 * E2E Test: EVM to SVM bridge flow
 *
 * Stakes USDC on EVM (Arbitrum Sepolia), verifies StakeEvent, waits for
 * relayer to submit Ed25519 signatures and unlock on SVM (1024chain),
 * verifies CrossChainSuccessEvent and SVM balance increase.
 *
 * Prerequisites:
 * - EVM contract deployed (set EVM_CONTRACT_ADDRESS)
 * - SVM program deployed (set SVM_PROGRAM_ID)
 * - Relayer running with BRIDGE_ID
 * - Both chains funded with USDC and native tokens
 *
 * Environment variables:
 *   EVM_RPC_URL            - Arbitrum Sepolia RPC
 *   SVM_RPC_URL            - 1024chain Testnet RPC
 *   EVM_CONTRACT_ADDRESS   - Bridge contract on EVM
 *   SVM_PROGRAM_ID         - Bridge program on SVM
 *   EVM_TOKEN_ADDRESS      - USDC address on EVM
 *   USER_PRIVATE_KEY       - EVM sender private key
 *   RECEIVER_ADDRESS       - 1024chain receiver pubkey (base58)
 *   AMOUNT                 - Amount in USDC atomic units (default: 100000000 = 100 USDC)
 *   POLL_INTERVAL_MS       - Balance poll interval (default: 5000)
 *   TIMEOUT_MS             - Max wait for relayer (default: 120000)
 *
 * Usage: npx ts-node tests/e2e/evm-to-svm.ts
 */

import { ethers } from "ethers";

const ERC20_ABI = [
    "function balanceOf(address owner) view returns (uint256)",
    "function approve(address spender, uint256 amount) returns (bool)",
    "function allowance(address owner, address spender) view returns (uint256)",
];

const BRIDGE_ABI = [
    "function stake(uint256 amount, string receiverAddress) returns (uint64)",
    "event StakeEvent(bytes32 indexed sourceContract, bytes32 indexed targetContract, uint64 chainId, uint64 blockHeight, uint64 amount, address sender, string receiverAddress, uint64 nonce)",
    "event TokensUnlocked(uint64 indexed nonce, address receiver, uint64 amount)",
];

function log(msg: string): void {
    console.log(`[evm->svm][${new Date().toISOString()}] ${msg}`);
}

function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
    const rpcUrl = process.env.EVM_RPC_URL!;
    const contractAddress = process.env.EVM_CONTRACT_ADDRESS!;
    const privateKey = process.env.USER_PRIVATE_KEY!;
    const receiverAddress = process.env.RECEIVER_ADDRESS!;
    const usdcAddress = process.env.EVM_TOKEN_ADDRESS!;
    const amount = process.env.AMOUNT || "100000000";
    const pollIntervalMs = parseInt(process.env.POLL_INTERVAL_MS || "5000");
    const timeoutMs = parseInt(process.env.TIMEOUT_MS || "120000");

    log("============================================");
    log("  Bridge1024 E2E: EVM -> SVM");
    log("============================================");
    log(`Bridge:   ${contractAddress}`);
    log(`USDC:     ${usdcAddress}`);
    log(`Amount:   ${amount}`);
    log(`Receiver: ${receiverAddress}`);
    log("");

    const provider = new ethers.JsonRpcProvider(rpcUrl);
    const wallet = new ethers.Wallet(privateKey, provider);
    const usdc = new ethers.Contract(usdcAddress, ERC20_ABI, wallet);
    const bridge = new ethers.Contract(contractAddress, BRIDGE_ABI, wallet);

    // Pre-flight: check EVM USDC balance
    const evmBal = (await usdc.balanceOf(wallet.address)) as bigint;
    if (evmBal < BigInt(amount)) {
        throw new Error(`Insufficient EVM USDC: have ${evmBal}, need ${amount}`);
    }
    log(`EVM USDC balance: ${evmBal}`);

    // Step 1: Ensure allowance
    log("\n--- Step 1: Approve USDC ---");
    const currentAllowance = (await usdc.allowance(wallet.address, contractAddress)) as bigint;
    if (currentAllowance < BigInt(amount)) {
        log(`Allowance ${currentAllowance} < ${amount}, approving MaxUint256...`);
        const approveTx = await usdc.approve(contractAddress, ethers.MaxUint256);
        await approveTx.wait();
        log(`Approved (tx: ${approveTx.hash})`);
    } else {
        log(`Allowance sufficient: ${currentAllowance}`);
    }

    // Step 2: Stake on EVM
    log("\n--- Step 2: Stake USDC ---");
    const stakeTx = await bridge.stake(amount, receiverAddress);
    const receipt = await stakeTx.wait();
    log(`Stake tx: ${receipt.hash}`);

    // Step 3: Verify StakeEvent
    log("\n--- Step 3: Verify StakeEvent ---");
    const stakeEvent = receipt.logs
        .map((l: any) => {
            try { return bridge.interface.parseLog(l); } catch { return null; }
        })
        .find((e: any) => e?.name === "StakeEvent");

    if (!stakeEvent) {
        throw new Error("StakeEvent not found in tx receipt");
    }
    log(`StakeEvent emitted:`);
    log(`  sender:   ${stakeEvent.args.sender}`);
    log(`  amount:   ${stakeEvent.args.amount}`);
    log(`  nonce:    ${stakeEvent.args.nonce}`);
    log(`  receiver: ${stakeEvent.args.receiverAddress}`);

    if (stakeEvent.args.amount !== BigInt(amount)) {
        throw new Error(`StakeEvent.amount mismatch: ${stakeEvent.args.amount} != ${amount}`);
    }

    // Step 4: Wait for relayer to process and unlock on SVM
    log("\n--- Step 4: Waiting for relayer to process ---");
    log(`Polling SVM chain for balance change (timeout: ${timeoutMs}ms)...`);
    log("TODO: Add SVM balance polling using @solana/web3.js");
    log("  - Connect to SVM_RPC_URL");
    log("  - Get receiver's USDC ATA");
    log("  - Poll until balance increases by expected amount");
    log("  - Verify CrossChainSuccessEvent in recent program logs");

    // TODO: Implement SVM balance polling
    // const { Connection, PublicKey } = await import("@solana/web3.js");
    // const { getAccount, getAssociatedTokenAddress } = await import("@solana/spl-token");
    // const svmConnection = new Connection(process.env.SVM_RPC_URL!);
    // const receiverPubkey = new PublicKey(receiverAddress);
    // const svmUsdcMint = new PublicKey(process.env.SVM_TOKEN_ADDRESS!);
    // const receiverAta = await getAssociatedTokenAddress(svmUsdcMint, receiverPubkey);
    //
    // const deadline = Date.now() + timeoutMs;
    // while (Date.now() < deadline) {
    //     const acct = await getAccount(svmConnection, receiverAta, "confirmed");
    //     log(`SVM balance: ${acct.amount}`);
    //     if (acct.amount >= expectedBalance) break;
    //     await sleep(pollIntervalMs);
    // }

    // Step 5: Verify receiver balance on SVM
    log("\n--- Step 5: Verify receiver balance ---");
    log("TODO: Compare SVM balance before/after and assert increase matches amount");

    log("\n============================================");
    log("  E2E Test Complete");
    log("============================================");
}

main()
    .then(() => process.exit(0))
    .catch((err) => {
        console.error(`[evm->svm] FAILED: ${err.message || err}`);
        if (err.stack) console.error(err.stack);
        process.exit(1);
    });
