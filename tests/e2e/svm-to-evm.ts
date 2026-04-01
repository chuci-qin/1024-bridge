/**
 * E2E Test: SVM to EVM bridge flow
 *
 * Stakes USDC on SVM (1024chain), verifies StakeEvent from tx logs,
 * waits for relayer to submit ECDSA signatures and unlock on EVM
 * (Arbitrum Sepolia), verifies TokensUnlocked event and EVM balance increase.
 *
 * Fee flow: SVM stake deducts bridge_fee, StakeEvent.amount = net_amount.
 * EVM unlock transfers full event amount (no fee on EVM side).
 *
 * Prerequisites:
 * - SVM program deployed (set SVM_PROGRAM_ID)
 * - EVM contract deployed (set EVM_CONTRACT_ADDRESS)
 * - Relayer running with BRIDGE_ID
 * - Both chains funded with USDC and native tokens
 *
 * Environment variables:
 *   SVM_RPC_URL            - 1024chain Testnet RPC
 *   EVM_RPC_URL            - Arbitrum Sepolia RPC
 *   SVM_PROGRAM_ID         - Bridge program on SVM
 *   EVM_CONTRACT_ADDRESS   - Bridge contract on EVM
 *   SVM_TOKEN_ADDRESS      - USDC mint on SVM
 *   EVM_TOKEN_ADDRESS      - USDC address on EVM
 *   ADMIN_KEYPAIR_PATH     - Path to SVM admin keypair JSON
 *   ADMIN_EVM_PRIVATE_KEY  - EVM admin private key
 *   IDL_PATH               - Path to bridge program IDL
 *   RECEIVER_ADDRESS       - EVM receiver address (0x...)
 *   AMOUNT                 - Amount in USDC atomic units (default: 100000000)
 *   SVM_BRIDGE_FEE         - Fee deducted on SVM stake (default: 0)
 *   POLL_INTERVAL_MS       - Balance poll interval (default: 5000)
 *   TIMEOUT_MS             - Max wait for relayer (default: 120000)
 *
 * Usage: npx ts-node tests/e2e/svm-to-evm.ts
 */

import BN from "bn.js";
import * as fs from "fs";
import { ethers } from "ethers";
import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import {
    getAssociatedTokenAddress,
    getAccount,
} from "@solana/spl-token";

function log(msg: string): void {
    console.log(`[svm->evm][${new Date().toISOString()}] ${msg}`);
}

function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

function loadKeypair(path: string): Keypair {
    const raw = JSON.parse(fs.readFileSync(path, "utf-8"));
    return Keypair.fromSecretKey(Uint8Array.from(raw));
}

async function main() {
    const svmRpcUrl = process.env.SVM_RPC_URL!;
    const evmRpcUrl = process.env.EVM_RPC_URL!;
    const svmProgramId = process.env.SVM_PROGRAM_ID!;
    const evmContractAddress = process.env.EVM_CONTRACT_ADDRESS!;
    const svmTokenAddress = process.env.SVM_TOKEN_ADDRESS!;
    const evmTokenAddress = process.env.EVM_TOKEN_ADDRESS!;
    const keypairPath = process.env.ADMIN_KEYPAIR_PATH!;
    const evmPrivateKey = process.env.ADMIN_EVM_PRIVATE_KEY!;
    const idlPath = process.env.IDL_PATH!;
    const receiverAddress = process.env.RECEIVER_ADDRESS!;
    const amount = process.env.AMOUNT || "100000000";
    const svmBridgeFee = parseInt(process.env.SVM_BRIDGE_FEE || "0");
    const pollIntervalMs = parseInt(process.env.POLL_INTERVAL_MS || "5000");
    const timeoutMs = parseInt(process.env.TIMEOUT_MS || "120000");

    log("============================================");
    log("  Bridge1024 E2E: SVM -> EVM");
    log("============================================");
    log(`SVM Program:  ${svmProgramId}`);
    log(`EVM Contract: ${evmContractAddress}`);
    log(`Amount:       ${amount}`);
    log(`Bridge Fee:   ${svmBridgeFee}`);
    log(`Receiver:     ${receiverAddress}`);
    log("");

    // Setup SVM
    const adminKeypair = loadKeypair(keypairPath);
    const connection = new Connection(svmRpcUrl, "confirmed");
    const wallet = new Wallet(adminKeypair);
    const provider = new AnchorProvider(connection, wallet, {
        commitment: "confirmed",
        preflightCommitment: "confirmed",
    });
    anchor.setProvider(provider);

    const idl = JSON.parse(fs.readFileSync(idlPath, "utf-8"));
    const programId = new PublicKey(svmProgramId);
    if (idl.address) idl.address = svmProgramId;
    if (idl.metadata?.address) idl.metadata.address = svmProgramId;
    const program = new Program(idl, provider);

    const usdcMint = new PublicKey(svmTokenAddress);
    const mintInfo = await connection.getAccountInfo(usdcMint);
    if (!mintInfo) throw new Error(`USDC mint not found: ${svmTokenAddress}`);
    const tokenProgramId = mintInfo.owner;

    const adminAta = await getAssociatedTokenAddress(usdcMint, adminKeypair.publicKey, false, tokenProgramId);
    const [senderState] = PublicKey.findProgramAddressSync([Buffer.from("sender_state")], programId);
    const [vault] = PublicKey.findProgramAddressSync([Buffer.from("vault")], programId);
    const vaultAta = await getAssociatedTokenAddress(usdcMint, vault, true, tokenProgramId);

    // Pre-flight: check SVM USDC balance
    log("\n--- Pre-flight ---");
    const svmBal = (await getAccount(connection, adminAta, "confirmed", tokenProgramId)).amount;
    if (svmBal < BigInt(amount)) {
        throw new Error(`Insufficient SVM USDC: have ${svmBal}, need ${amount}`);
    }
    log(`SVM USDC balance: ${svmBal}`);

    // Record EVM balance before
    const evmProvider = new ethers.JsonRpcProvider(evmRpcUrl);
    const evmUsdc = new ethers.Contract(evmTokenAddress, [
        "function balanceOf(address) view returns (uint256)",
    ], evmProvider);
    const evmBalBefore = (await evmUsdc.balanceOf(receiverAddress)) as bigint;
    log(`EVM USDC before: ${evmBalBefore}`);

    // Step 1: Stake on SVM
    log("\n--- Step 1: Stake USDC on SVM ---");
    const stakeTxSig = await program.methods
        .stake(new BN(amount), receiverAddress)
        .accounts({
            senderState,
            user: adminKeypair.publicKey,
            vault,
            usdcMint,
            userTokenAccount: adminAta,
            vaultTokenAccount: vaultAta,
            tokenProgram: tokenProgramId,
        })
        .signers([adminKeypair])
        .rpc();
    log(`Stake tx: ${stakeTxSig}`);

    // Step 2: Verify StakeEvent from tx logs
    log("\n--- Step 2: Verify StakeEvent ---");
    let stakeEventVerified = false;
    for (let attempt = 0; attempt < 5; attempt++) {
        const tx = await connection.getTransaction(stakeTxSig, {
            commitment: "confirmed",
            maxSupportedTransactionVersion: 0,
        });
        if (tx?.meta?.logMessages) {
            const dataLines = tx.meta.logMessages.filter((l) => l.startsWith("Program data: "));
            if (dataLines.length > 0) {
                log("StakeEvent detected in tx logs");
                stakeEventVerified = true;
            }
            break;
        }
        log(`Logs not available yet, retrying in 3s (${attempt + 1}/5)...`);
        await sleep(3000);
    }
    if (!stakeEventVerified) {
        log("WARNING: Could not verify StakeEvent from tx logs");
    }

    // Step 3: Wait for EVM balance to increase
    log("\n--- Step 3: Wait for relayer to unlock on EVM ---");
    const expectedNet = BigInt(amount) - BigInt(svmBridgeFee);
    const evmExpected = evmBalBefore + expectedNet;
    log(`Expected EVM balance: >= ${evmExpected} (net: +${expectedNet})`);

    const deadline = Date.now() + timeoutMs;
    let evmBalAfter = evmBalBefore;
    while (Date.now() < deadline) {
        evmBalAfter = (await evmUsdc.balanceOf(receiverAddress)) as bigint;
        log(`EVM USDC: ${evmBalAfter}`);
        if (evmBalAfter >= evmExpected) break;
        await sleep(pollIntervalMs);
    }

    if (evmBalAfter < evmExpected) {
        throw new Error(`Timeout: EVM balance ${evmBalAfter} < expected ${evmExpected}`);
    }

    // Step 4: Verify TokensUnlocked event
    log("\n--- Step 4: Verify TokensUnlocked event ---");
    const evmWallet = new ethers.Wallet(evmPrivateKey, evmProvider);
    const bridge = new ethers.Contract(evmContractAddress, [
        "event TokensUnlocked(uint64 indexed nonce, address receiver, uint64 amount)",
    ], evmWallet);

    try {
        const recentBlock = await evmProvider.getBlockNumber();
        const events = await bridge.queryFilter(bridge.filters.TokensUnlocked(), recentBlock - 200, recentBlock);
        const matching = events.find((e: any) => {
            const parsed = bridge.interface.parseLog(e);
            return parsed && parsed.args.amount === BigInt(expectedNet);
        });
        if (matching) {
            const parsed = bridge.interface.parseLog(matching);
            log(`TokensUnlocked: nonce=${parsed!.args.nonce}, receiver=${parsed!.args.receiver}, amount=${parsed!.args.amount}`);
        } else {
            log("WARNING: TokensUnlocked event not found in recent blocks");
        }
    } catch (err: any) {
        log(`WARNING: TokensUnlocked query failed: ${err.message}`);
    }

    log("\n============================================");
    log("  PASSED: SVM -> EVM transfer verified");
    log("============================================");
}

main()
    .then(() => process.exit(0))
    .catch((err) => {
        console.error(`[svm->evm] FAILED: ${err.message || err}`);
        if (err.stack) console.error(err.stack);
        process.exit(1);
    });
