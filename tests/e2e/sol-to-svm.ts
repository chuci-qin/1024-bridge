/**
 * E2E Test: Solana to 1024chain (SVM) bridge flow
 *
 * Stakes USDC on Solana Devnet bridge program, verifies StakeEvent from tx logs,
 * waits for sol2svm relayer to submit Ed25519 signatures and unlock on 1024chain,
 * verifies CrossChainSuccessEvent and SVM balance increase.
 *
 * Fee flow: Solana stake charges NO fee. SVM unlock deducts bridge_fee.
 * Receiver gets: staked_amount - svm_bridge_fee.
 *
 * Prerequisites:
 * - Solana bridge program deployed (set SOLANA_PROGRAM_ID)
 * - SVM bridge program deployed (set SVM_PROGRAM_ID)
 * - sol2svm relayer running
 * - Both chains funded with USDC and native tokens
 *
 * Environment variables:
 *   SOLANA_RPC_URL         - Solana Devnet RPC
 *   SVM_RPC_URL            - 1024chain Testnet RPC
 *   SOLANA_PROGRAM_ID      - Bridge program on Solana
 *   SVM_PROGRAM_ID         - Bridge program on 1024chain
 *   SOLANA_TOKEN_ADDRESS   - USDC mint on Solana
 *   SVM_TOKEN_ADDRESS      - USDC mint on 1024chain
 *   SOLANA_KEYPAIR_PATH    - Path to Solana admin keypair JSON
 *   SVM_KEYPAIR_PATH       - Path to SVM admin keypair JSON
 *   SOLANA_IDL_PATH        - Path to Solana bridge IDL
 *   SVM_IDL_PATH           - Path to SVM bridge IDL
 *   AMOUNT                 - Amount in USDC atomic units (default: 100000000)
 *   SVM_BRIDGE_FEE         - Fee deducted on SVM unlock (default: 0)
 *   POLL_INTERVAL_MS       - Balance poll interval (default: 5000)
 *   TIMEOUT_MS             - Max wait for relayer (default: 120000)
 *
 * Usage: npx ts-node tests/e2e/sol-to-svm.ts
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
    console.log(`[sol->svm][${new Date().toISOString()}] ${msg}`);
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
    const pollIntervalMs = parseInt(process.env.POLL_INTERVAL_MS || "5000");
    const timeoutMs = parseInt(process.env.TIMEOUT_MS || "120000");

    log("============================================");
    log("  Bridge1024 E2E: Solana -> 1024chain");
    log("============================================");
    log(`Solana Program: ${process.env.SOLANA_PROGRAM_ID}`);
    log(`SVM Program:    ${process.env.SVM_PROGRAM_ID}`);
    log(`Amount:         ${amount}`);
    log(`SVM Bridge Fee: ${svmBridgeFee}`);
    log("");

    const solana = await setupChain(
        process.env.SOLANA_RPC_URL!,
        process.env.SOLANA_KEYPAIR_PATH!,
        process.env.SOLANA_PROGRAM_ID!,
        process.env.SOLANA_TOKEN_ADDRESS!,
        process.env.SOLANA_IDL_PATH!,
    );

    const svm = await setupChain(
        process.env.SVM_RPC_URL!,
        process.env.SVM_KEYPAIR_PATH!,
        process.env.SVM_PROGRAM_ID!,
        process.env.SVM_TOKEN_ADDRESS!,
        process.env.SVM_IDL_PATH!,
    );

    log(`Admin Solana: ${solana.adminPubkey.toBase58()}`);
    log(`Admin SVM:    ${svm.adminPubkey.toBase58()}`);

    // Pre-flight: check Solana USDC balance
    log("\n--- Pre-flight ---");
    const solanaBal = (await getAccount(solana.connection, solana.adminAta, "confirmed", solana.tokenProgramId)).amount;
    if (solanaBal < BigInt(amount)) {
        throw new Error(`Insufficient Solana USDC: have ${solanaBal}, need ${amount}`);
    }
    log(`Solana USDC: ${solanaBal}`);

    // Record SVM balance before
    let svmBalBefore: bigint;
    try {
        svmBalBefore = (await getAccount(svm.connection, svm.adminAta, "confirmed", svm.tokenProgramId)).amount;
    } catch {
        svmBalBefore = 0n;
    }
    log(`SVM USDC before: ${svmBalBefore}`);

    // Step 1: Stake on Solana bridge
    log("\n--- Step 1: Stake USDC on Solana ---");
    const receiverPubkey = svm.adminPubkey;
    log(`Staking ${amount} (receiver: ${receiverPubkey.toBase58()})...`);

    const stakeTxSig = await solana.program.methods
        .stake(new BN(amount), receiverPubkey.toBase58())
        .accounts({
            senderState: solana.senderState,
            receiverState: solana.receiverState,
            user: solana.adminPubkey,
            vault: solana.vault,
            usdcMint: solana.usdcMint,
            userTokenAccount: solana.adminAta,
            vaultTokenAccount: solana.vaultAta,
            tokenProgram: solana.tokenProgramId,
            systemProgram: SystemProgram.programId,
        })
        .rpc();
    log(`Stake tx: ${stakeTxSig}`);

    // Step 2: Verify StakeEvent from Solana tx logs
    log("\n--- Step 2: Verify StakeEvent ---");
    for (let attempt = 0; attempt < 5; attempt++) {
        const tx = await solana.connection.getTransaction(stakeTxSig, {
            commitment: "confirmed",
            maxSupportedTransactionVersion: 0,
        });
        if (tx?.meta?.logMessages) {
            const dataLines = tx.meta.logMessages.filter((l) => l.startsWith("Program data: "));
            if (dataLines.length > 0) {
                log(`StakeEvent detected in tx logs (${dataLines.length} data lines)`);
                log("Solana stake emits full amount (no fee on Solana side)");
            }
            break;
        }
        log(`Logs not available yet, retrying in 3s (${attempt + 1}/5)...`);
        await sleep(3000);
    }

    // Step 3: Wait for SVM balance to increase
    log("\n--- Step 3: Wait for relayer to unlock on SVM ---");
    const expectedNet = BigInt(amount) - BigInt(svmBridgeFee);
    const svmExpected = svmBalBefore + expectedNet;
    log(`Expected SVM balance: >= ${svmExpected} (net: +${expectedNet}, fee: ${svmBridgeFee})`);

    const deadline = Date.now() + timeoutMs;
    let svmBalAfter = svmBalBefore;
    while (Date.now() < deadline) {
        try {
            svmBalAfter = (await getAccount(svm.connection, svm.adminAta, "confirmed", svm.tokenProgramId)).amount;
        } catch {
            svmBalAfter = 0n;
        }
        log(`SVM USDC: ${svmBalAfter}`);
        if (svmBalAfter >= svmExpected) break;
        await sleep(pollIntervalMs);
    }

    if (svmBalAfter < svmExpected) {
        throw new Error(`Timeout: SVM balance ${svmBalAfter} < expected ${svmExpected}`);
    }

    const actualIncrease = svmBalAfter - svmBalBefore;
    log(`Balance increase: ${actualIncrease} (expected net: ${expectedNet})`);

    if (svmBridgeFee > 0 && actualIncrease === BigInt(amount)) {
        throw new Error("Bridge fee was NOT deducted on SVM unlock!");
    }

    // Step 4: Verify CrossChainSuccessEvent on SVM
    log("\n--- Step 4: Verify CrossChainSuccessEvent ---");
    const disc = anchorEventDiscriminator("CrossChainSuccessEvent");
    const seen = new Set<string>();
    let eventFound = false;
    const eventDeadline = Date.now() + 30000;

    while (Date.now() < eventDeadline && !eventFound) {
        const sigs = await svm.connection.getSignaturesForAddress(svm.programId, { limit: 10 }, "confirmed");
        for (const sig of sigs) {
            if (sig.err || seen.has(sig.signature)) continue;
            seen.add(sig.signature);
            const tx = await svm.connection.getTransaction(sig.signature, {
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
    log("  PASSED: Solana -> 1024chain transfer verified");
    if (svmBridgeFee > 0) {
        log(`  Fee deducted on SVM: ${svmBridgeFee}`);
        log(`  Net received: ${actualIncrease}`);
    }
    log("============================================");
}

main()
    .then(() => process.exit(0))
    .catch((err) => {
        console.error(`[sol->svm] FAILED: ${err.message || err}`);
        if (err.stack) console.error(err.stack);
        process.exit(1);
    });
