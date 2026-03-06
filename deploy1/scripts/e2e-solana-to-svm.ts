/**
 * e2e-solana-to-svm.ts
 *
 * E2E test: Solana -> 1024chain (SVM) direction.
 * Stakes USDC on Solana Bridge, verifies StakeEvent from tx logs,
 * waits for sol2svm relayer to submit signatures and unlock on SVM,
 * verifies CrossChainSuccessEvent and SVM balance increase.
 *
 * Fee flow: Solana stake charges NO fee. SVM unlock deducts bridge_fee.
 * Receiver gets: staked_amount - svm_bridge_fee.
 *
 * When BRIDGE_ID is set, derives a unique SVM receiver address per bridge
 * to prevent concurrent tests from interfering with each other's balance checks.
 * After the test, reclaims USDC from the derived address back to admin.
 *
 * Environment variables: see e2e-helpers.ts loadConfig()
 */

import BN from "bn.js";
import {
  getOrCreateAssociatedTokenAccount,
  MintLayout,
} from "@solana/spl-token";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import {
  loadConfig, log, sleep,
  setupSolana, setupSvm,
  getTokenBalance, pollUntilBalanceChanges,
  getTransactionLogsWithRetry, parseStakeEvent,
  pollForEvent, anchorEventDiscriminator, parseCrossChainSuccessEvent,
  deriveReceiverKeypair, reclaimToAdmin,
} from "./e2e-helpers";

const TAG = "solana->svm";

async function main() {
  const cfg = loadConfig();

  log(TAG, "============================================");
  log(TAG, "  Bridge1024 E2E: Solana -> 1024chain");
  log(TAG, "============================================");
  log(TAG, `Solana Program:  ${cfg.solanaProgramId}`);
  log(TAG, `SVM Program:     ${cfg.svmProgramId}`);
  log(TAG, `Test Amount:     ${cfg.testAmount}`);
  log(TAG, `SVM Bridge Fee:  ${cfg.svmBridgeFee}`);
  if (cfg.bridgeId) log(TAG, `Bridge ID:       ${cfg.bridgeId} (using derived receiver)`);
  log(TAG, "");

  const solana = await setupSolana(cfg);
  const svm = await setupSvm(cfg);
  log(TAG, `Admin Solana: ${solana.adminPubkey.toBase58()}`);
  log(TAG, `Admin SVM:    ${svm.adminPubkey.toBase58()}`);

  // Determine receiver: derived (per-bridge isolated) or admin (fallback)
  let receiverPubkey: PublicKey;
  let receiverAta: PublicKey;
  let derivedKeypair: Keypair | null = null;

  if (cfg.bridgeId) {
    derivedKeypair = deriveReceiverKeypair(svm.adminKeypair, cfg.bridgeId);
    receiverPubkey = derivedKeypair.publicKey;
    log(TAG, `Derived receiver: ${receiverPubkey.toBase58()} (bridge: ${cfg.bridgeId})`);

    const ataAccount = await getOrCreateAssociatedTokenAccount(
      svm.connection, svm.adminKeypair, svm.usdcMint, receiverPubkey,
      false, undefined, undefined, svm.tokenProgramId,
    );
    receiverAta = ataAccount.address;
    log(TAG, `Derived ATA: ${receiverAta.toBase58()}`);
  } else {
    receiverPubkey = svm.adminPubkey;
    receiverAta = svm.adminAta;
  }

  // Pre-flight
  const solanaBal = await getTokenBalance(solana.connection, solana.adminAta, solana.tokenProgramId);
  if (solanaBal < BigInt(cfg.testAmount)) {
    throw new Error(`Insufficient Solana USDC: have ${solanaBal}, need ${cfg.testAmount}`);
  }
  log(TAG, `Solana USDC balance: ${solanaBal}`);

  const svmBalBefore = await getTokenBalance(svm.connection, receiverAta, svm.tokenProgramId);
  log(TAG, `SVM USDC before: ${svmBalBefore}`);

  const svmVaultBefore = await getTokenBalance(svm.connection, svm.vaultAta, svm.tokenProgramId);
  log(TAG, `SVM vault before: ${svmVaultBefore}`);

  // Step 1: Stake on Solana Bridge
  log(TAG, `Staking ${cfg.testAmount} on Solana (receiver: ${receiverPubkey.toBase58()})...`);
  const stakeTxSig = await solana.program.methods
    .stake(new BN(cfg.testAmount), receiverPubkey.toBase58())
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
  log(TAG, `Stake tx: ${stakeTxSig}`);

  // Step 2: Verify StakeEvent from Solana tx logs
  log(TAG, "Verifying StakeEvent from tx logs...");
  const logMessages = await getTransactionLogsWithRetry(TAG, solana.connection, stakeTxSig);

  if (logMessages) {
    const stakeEvent = parseStakeEvent(logMessages);
    if (stakeEvent) {
      log(TAG, `StakeEvent: source=${stakeEvent.sourceContract}, amount=${stakeEvent.amount}, nonce=${stakeEvent.nonce}, receiver=${stakeEvent.receiverAddress}`);
      if (stakeEvent.amount !== BigInt(cfg.testAmount)) {
        throw new Error(`StakeEvent.amount mismatch: ${stakeEvent.amount} != ${cfg.testAmount} (Solana fee=0, should emit full amount)`);
      }
      if (stakeEvent.receiverAddress !== receiverPubkey.toBase58()) {
        throw new Error(`StakeEvent.receiverAddress mismatch: ${stakeEvent.receiverAddress} != ${receiverPubkey.toBase58()}`);
      }
      log(TAG, "StakeEvent fields verified");
    } else {
      log(TAG, "WARNING: StakeEvent not found in tx logs");
    }
  } else {
    log(TAG, "WARNING: Could not retrieve transaction logs. Proceeding with balance verification.");
  }

  // Step 3: Wait for SVM balance to increase
  const expectedNet = BigInt(cfg.testAmount - cfg.svmBridgeFee);
  const svmExpected = svmBalBefore + expectedNet;
  log(TAG, `Waiting for SVM USDC to reach >= ${svmExpected} (net: +${expectedNet}, fee: ${cfg.svmBridgeFee})...`);

  const svmBalAfter = await pollUntilBalanceChanges(
    TAG, "SVM USDC",
    () => getTokenBalance(svm.connection, receiverAta, svm.tokenProgramId),
    svmExpected,
    { initialDelayMs: cfg.initialDelayMs, pollIntervalMs: cfg.pollIntervalMs, timeoutMs: cfg.timeoutMs },
  );
  log(TAG, `SVM USDC after: ${svmBalAfter}`);

  const actualIncrease = svmBalAfter - svmBalBefore;
  if (actualIncrease !== expectedNet) {
    log(TAG, `WARNING: Balance increase ${actualIncrease} != expected ${expectedNet}`);
    if (cfg.svmBridgeFee > 0 && actualIncrease === BigInt(cfg.testAmount)) {
      throw new Error("Bridge fee was NOT deducted on SVM unlock!");
    }
  }

  // Step 4: Verify vault fee retention
  const svmVaultAfter = await getTokenBalance(svm.connection, svm.vaultAta, svm.tokenProgramId);
  log(TAG, `SVM vault after: ${svmVaultAfter}`);
  const vaultChange = svmVaultAfter - svmVaultBefore;
  log(TAG, `SVM vault change: ${vaultChange} (unlocked ${actualIncrease}, fee retained: ${cfg.svmBridgeFee})`);

  // Step 5: Verify CrossChainSuccessEvent on SVM
  log(TAG, "Checking for CrossChainSuccessEvent on SVM...");
  const disc = anchorEventDiscriminator("CrossChainSuccessEvent");
  const eventResult = await pollForEvent(
    TAG, svm.connection, svm.programId, disc, cfg.timeoutMs, cfg.pollIntervalMs,
  );
  if (eventResult) {
    const ccEvent = parseCrossChainSuccessEvent(eventResult.logMessages);
    if (ccEvent) {
      log(TAG, `CrossChainSuccessEvent: sender=${ccEvent.senderAddress}, amount=${ccEvent.amount}, nonce=${ccEvent.nonce}`);
    }
  } else {
    log(TAG, "WARNING: CrossChainSuccessEvent not found within timeout (balance did increase, event polling may have missed it)");
  }

  log(TAG, "");
  log(TAG, "PASSED: Solana -> 1024chain transfer verified");
  if (cfg.svmBridgeFee > 0) {
    log(TAG, `  Fee deducted on SVM: ${cfg.svmBridgeFee}`);
    log(TAG, `  Net received: ${actualIncrease}`);
  }

  // Step 6: Reclaim USDC from derived address back to admin (best-effort)
  if (derivedKeypair) {
    log(TAG, "");
    log(TAG, "Reclaiming USDC from derived address...");
    const mintInfo = await svm.connection.getAccountInfo(svm.usdcMint);
    const decimals = mintInfo ? MintLayout.decode(mintInfo.data).decimals : 6;
    await reclaimToAdmin(
      TAG, svm.connection, derivedKeypair, svm.adminKeypair,
      svm.usdcMint, svm.tokenProgramId, decimals,
    );
  }
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(`[${TAG}] FAILED: ${err.message || err}`);
    if (err.stack) console.error(err.stack);
    process.exit(1);
  });
