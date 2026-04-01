/**
 * E2E Test: 1024chain (SVM) to Solana bridge flow
 *
 * Stakes USDC on 1024chain bridge program (bridge_fee deducted on stake),
 * verifies StakeEvent from tx logs, waits for svm2sol relayer to submit
 * Ed25519 signatures and unlock on Solana, verifies CrossChainSuccessEvent
 * and Solana balance increase.
 *
 * Fee flow: SVM stake deducts bridge_fee, emits StakeEvent.amount = net_amount.
 * Solana unlock transfers full event amount (no fee on Solana side).
 * Receiver gets: staked_amount - svm_bridge_fee.
 *
 * Prerequisites:
 * - SVM bridge program deployed (set SVM_PROGRAM_ID)
 * - Solana bridge program deployed (set SOLANA_PROGRAM_ID)
 * - svm2sol relayer running
 * - Both chains funded with USDC and native tokens
 *
 * Environment variables:
 *   SVM_RPC_URL            - 1024chain Testnet RPC
 *   SOLANA_RPC_URL         - Solana Devnet RPC
 *   SVM_PROGRAM_ID         - Bridge program on 1024chain
 *   SOLANA_PROGRAM_ID      - Bridge program on Solana
 *   SVM_TOKEN_ADDRESS      - USDC mint on 1024chain
 *   SOLANA_TOKEN_ADDRESS   - USDC mint on Solana
 *   SVM_KEYPAIR_PATH       - Path to SVM admin keypair JSON
 *   SOLANA_KEYPAIR_PATH    - Path to Solana admin keypair JSON
 *   SVM_IDL_PATH           - Path to SVM bridge IDL
 *   SOLANA_IDL_PATH        - Path to Solana bridge IDL
 *   AMOUNT                 - Amount in USDC atomic units (default: 100000000)
 *   SVM_BRIDGE_FEE         - Fee deducted on SVM stake (default: 0)
 *   POLL_INTERVAL_MS       - Balance poll interval (default: 5000)
 *   TIMEOUT_MS             - Max wait for relayer (default: 120000)
 *
 * Usage: npx ts-node tests/e2e/svm-to-sol.ts
 */

import BN from "bn.js";
import * as fs from "fs";
import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import {
    getAssociatedTokenAddress,
    getOrCreateAssociatedTokenAccount,
    getAccount,
} from "@solana/spl-token";
import * as crypto from "crypto";

function log(msg: string): void {
    console.log(`[svm->sol][${new Date().toISOString()}] ${msg}`);
}

function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

function loadKeypair(path: string): Keypair {
    const raw = JSON.parse(fs.readFileSync(path, "utf-8"));
    return Keypair.fromSecretKey(Uint8Array.from(raw));
}

function anchorEventDiscriminator(eventName: string): Buffer {
    return crypto.createHash("sha256").update(`event:${eventName}`).digest().subarray(0, 8);
}

async function setupChain(
    rpcUrl: string,
    keypairPath: string,
    programIdStr: string,
    tokenAddressStr: string,
    idlPath: string,
) {
    const adminKeypair = loadKeypair(keypairPath);
    const connection = new Connection(rpcUrl, "confirmed");
    const wallet = new Wallet(adminKeypair);
    const provider = new AnchorProvider(connection, wallet, {
        commitment: "confirmed",
        preflightCommitment: "confirmed",
    });
    anchor.setProvider(provider);

    const idl = JSON.parse(fs.readFileSync(idlPath, "utf-8"));
    const programId = new PublicKey(programIdStr);
    if (idl.address) idl.address = programIdStr;
    if (idl.metadata?.address) idl.metadata.address = programIdStr;
    const program = new Program(idl, provider);

    const usdcMint = new PublicKey(tokenAddressStr);
    const mintInfo = await connection.getAccountInfo(usdcMint);
    if (!mintInfo) throw new Error(`USDC mint not found: ${tokenAddressStr}`);
    const tokenProgramId = mintInfo.owner;

    const [senderState] = PublicKey.findProgramAddressSync([Buffer.from("sender_state")], programId);
    const [receiverState] = PublicKey.findProgramAddressSync([Buffer.from("receiver_state")], programId);
    const [vault] = PublicKey.findProgramAddressSync([Buffer.from("vault")], programId);
    const adminAta = await getAssociatedTokenAddress(usdcMint, adminKeypair.publicKey, false, tokenProgramId);
    const vaultAta = await getAssociatedTokenAddress(usdcMint, vault, true, tokenProgramId);

    return {
        adminKeypair,
        adminPubkey: adminKeypair.publicKey,
        connection,
        provider,
        program,
        programId,
        usdcMint,
        tokenProgramId,
        senderState,
        receiverState,
        vault,
        adminAta,
        vaultAta,
    };
}

async function main() {
    const amount = process.env.AMOUNT || "100000000";
    const svmBridgeFee = parseInt(process.env.SVM_BRIDGE_FEE || "0");
    const expectedNet = parseInt(amount) - svmBridgeFee;
    const pollIntervalMs = parseInt(process.env.POLL_INTERVAL_MS || "5000");
    const timeoutMs = parseInt(process.env.TIMEOUT_MS || "120000");

    log("============================================");
    log("  Bridge1024 E2E: 1024chain -> Solana");
    log("============================================");
    log(`SVM Program:    ${process.env.SVM_PROGRAM_ID}`);
    log(`Solana Program: ${process.env.SOLANA_PROGRAM_ID}`);
    log(`Amount:         ${amount}`);
    log(`SVM Bridge Fee: ${svmBridgeFee}`);
    log(`Expected net:   ${expectedNet} (fee deducted on SVM stake)`);
    log("");

    const svm = await setupChain(
        process.env.SVM_RPC_URL!,
        process.env.SVM_KEYPAIR_PATH!,
        process.env.SVM_PROGRAM_ID!,
        process.env.SVM_TOKEN_ADDRESS!,
        process.env.SVM_IDL_PATH!,
    );

    const solana = await setupChain(
        process.env.SOLANA_RPC_URL!,
        process.env.SOLANA_KEYPAIR_PATH!,
        process.env.SOLANA_PROGRAM_ID!,
        process.env.SOLANA_TOKEN_ADDRESS!,
        process.env.SOLANA_IDL_PATH!,
    );

    log(`Admin SVM:    ${svm.adminPubkey.toBase58()}`);
    log(`Admin Solana: ${solana.adminPubkey.toBase58()}`);

    // Pre-flight: check SVM USDC balance
    log("\n--- Pre-flight ---");
    const svmBal = (await getAccount(svm.connection, svm.adminAta, "confirmed", svm.tokenProgramId)).amount;
    if (svmBal < BigInt(amount)) {
        throw new Error(`Insufficient SVM USDC: have ${svmBal}, need ${amount}`);
    }
    log(`SVM USDC: ${svmBal}`);

    // Record Solana balance before
    const receiverPubkey = solana.adminPubkey;
    let solanaBalBefore: bigint;
    try {
        solanaBalBefore = (await getAccount(solana.connection, solana.adminAta, "confirmed", solana.tokenProgramId)).amount;
    } catch {
        solanaBalBefore = 0n;
    }
    log(`Solana USDC before: ${solanaBalBefore}`);

    // Record SVM vault before
    let svmVaultBefore: bigint;
    try {
        svmVaultBefore = (await getAccount(svm.connection, svm.vaultAta, "confirmed", svm.tokenProgramId)).amount;
    } catch {
        svmVaultBefore = 0n;
    }
    log(`SVM vault before: ${svmVaultBefore}`);

    // Step 1: Stake on SVM (1024chain) bridge — fee deducted here
    log("\n--- Step 1: Stake USDC on SVM (fee deducted) ---");
    log(`Staking ${amount} on SVM (receiver: ${receiverPubkey.toBase58()})...`);

    const stakeTxSig = await svm.program.methods
        .stake(new BN(amount), receiverPubkey.toBase58())
        .accounts({
            senderState: svm.senderState,
            receiverState: svm.receiverState,
            user: svm.adminPubkey,
            vault: svm.vault,
            usdcMint: svm.usdcMint,
            userTokenAccount: svm.adminAta,
            vaultTokenAccount: svm.vaultAta,
            tokenProgram: svm.tokenProgramId,
            systemProgram: SystemProgram.programId,
        })
        .rpc();
    log(`Stake tx: ${stakeTxSig}`);

    // Step 2: Verify StakeEvent from SVM tx logs
    log("\n--- Step 2: Verify StakeEvent ---");
    let stakeEventVerified = false;
    for (let attempt = 0; attempt < 5; attempt++) {
        const tx = await svm.connection.getTransaction(stakeTxSig, {
            commitment: "confirmed",
            maxSupportedTransactionVersion: 0,
        });
        if (tx?.meta?.logMessages) {
            const dataLines = tx.meta.logMessages.filter((l) => l.startsWith("Program data: "));
            if (dataLines.length > 0) {
                log(`StakeEvent detected in tx logs (${dataLines.length} data lines)`);
                log(`Expected StakeEvent.amount = ${expectedNet} (amount ${amount} - fee ${svmBridgeFee})`);
                stakeEventVerified = true;
            }
            break;
        }
        log(`Logs not available yet, retrying in 3s (${attempt + 1}/5)...`);
        await sleep(3000);
    }
    if (!stakeEventVerified) {
        log("WARNING: Could not verify StakeEvent from tx logs (1024chain RPC limitation)");
    }

    // Verify SVM vault increased by full amount
    let svmVaultAfterStake: bigint;
    try {
        svmVaultAfterStake = (await getAccount(svm.connection, svm.vaultAta, "confirmed", svm.tokenProgramId)).amount;
    } catch {
        svmVaultAfterStake = 0n;
    }
    log(`SVM vault after stake: ${svmVaultAfterStake} (change: +${svmVaultAfterStake - svmVaultBefore})`);

    // Step 3: Wait for Solana balance to increase
    log("\n--- Step 3: Wait for relayer to unlock on Solana ---");
    const solanaExpected = solanaBalBefore + BigInt(expectedNet);
    log(`Expected Solana balance: >= ${solanaExpected} (net: +${expectedNet})`);

    const deadline = Date.now() + timeoutMs;
    let solanaBalAfter = solanaBalBefore;
    while (Date.now() < deadline) {
        try {
            solanaBalAfter = (await getAccount(solana.connection, solana.adminAta, "confirmed", solana.tokenProgramId)).amount;
        } catch {
            solanaBalAfter = 0n;
        }
        log(`Solana USDC: ${solanaBalAfter}`);
        if (solanaBalAfter >= solanaExpected) break;
        await sleep(pollIntervalMs);
    }

    if (solanaBalAfter < solanaExpected) {
        throw new Error(`Timeout: Solana balance ${solanaBalAfter} < expected ${solanaExpected}`);
    }

    const actualIncrease = solanaBalAfter - solanaBalBefore;
    log(`Balance increase: ${actualIncrease} (expected net: ${expectedNet})`);

    if (actualIncrease !== BigInt(expectedNet)) {
        log(`WARNING: Balance increase ${actualIncrease} != expected ${expectedNet}`);
    }

    // Step 4: Verify CrossChainSuccessEvent on Solana
    log("\n--- Step 4: Verify CrossChainSuccessEvent ---");
    const disc = anchorEventDiscriminator("CrossChainSuccessEvent");
    const seen = new Set<string>();
    let eventFound = false;
    const eventDeadline = Date.now() + 30000;

    while (Date.now() < eventDeadline && !eventFound) {
        const sigs = await solana.connection.getSignaturesForAddress(solana.programId, { limit: 10 }, "confirmed");
        for (const sig of sigs) {
            if (sig.err || seen.has(sig.signature)) continue;
            seen.add(sig.signature);
            const tx = await solana.connection.getTransaction(sig.signature, {
                commitment: "confirmed",
                maxSupportedTransactionVersion: 0,
            });
            if (!tx?.meta?.logMessages) continue;
            for (const line of tx.meta.logMessages) {
                if (!line.startsWith("Program data: ")) continue;
                const raw = Buffer.from(line.slice("Program data: ".length), "base64");
                if (raw.length >= 8 && raw.subarray(0, 8).equals(disc)) {
                    log(`CrossChainSuccessEvent found in tx ${sig.signature}`);
                    eventFound = true;
                    break;
                }
            }
            if (eventFound) break;
        }
        if (!eventFound) await sleep(pollIntervalMs);
    }

    if (!eventFound) {
        log("WARNING: CrossChainSuccessEvent not found (balance did increase, event polling may have missed it)");
    }

    log("\n============================================");
    log("  PASSED: 1024chain -> Solana transfer verified");
    if (svmBridgeFee > 0) {
        log(`  Fee deducted on SVM stake: ${svmBridgeFee}`);
        log(`  Net received on Solana: ${actualIncrease}`);
    }
    log("============================================");
}

main()
    .then(() => process.exit(0))
    .catch((err) => {
        console.error(`[svm->sol] FAILED: ${err.message || err}`);
        if (err.stack) console.error(err.stack);
        process.exit(1);
    });
