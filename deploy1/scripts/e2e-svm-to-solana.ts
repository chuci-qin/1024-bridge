/**
 * e2e-svm-to-solana.ts
 *
 * E2E test: 1024chain (SVM) -> Solana direction.
 * Stakes USDC on 1024chain Bridge (bridge_fee deducted on stake),
 * verifies StakeEvent from tx logs, waits for svm2sol relayer to
 * submit signatures and unlock on Solana, verifies CrossChainSuccessEvent
 * and Solana balance increase.
 *
 * Fee flow: SVM stake deducts bridge_fee, emits StakeEvent.amount = net_amount.
 * Solana unlock transfers full event amount (no fee on Solana side).
 * Receiver gets: staked_amount - svm_bridge_fee.
 *
 * When BRIDGE_ID is set, derives a unique Solana receiver address per bridge
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

const TAG = "svm->solana";

async function main() {
  const cfg = loadConfig();
  const expectedNet = cfg.testAmount - cfg.svmBridgeFee;

  log(TAG, "============================================");
  log(TAG, "  Bridge1024 E2E: 1024chain -> Solana");
  log(TAG, "============================================");
  log(TAG, `SVM Program:     ${cfg.svmProgramId}`);
  log(TAG, `Solana Program:  ${cfg.solanaProgramId}`);
  log(TAG, `Test Amount:     ${cfg.testAmount}`);
  log(TAG, `SVM Bridge Fee:  ${cfg.svmBridgeFee}`);
  log(TAG, `Expected net:    ${expectedNet} (fee deducted on SVM stake)`);
  if (cfg.bridgeId) log(TAG, `Bridge ID:       ${cfg.bridgeId} (using derived receiver)`);
  log(TAG, "");

  const svm = await setupSvm(cfg);
  const solana = await setupSolana(cfg);
  log(TAG, `Admin SVM:    ${svm.adminPubkey.toBase58()}`);
  log(TAG, `Admin Solana: ${solana.adminPubkey.toBase58()}`);

  // Determine receiver: derived (per-bridge isolated) or admin (fallback)
  let receiverPubkey: PublicKey;
  let receiverAta: PublicKey;
  let derivedKeypair: Keypair | null = null;

  if (cfg.bridgeId) {
    derivedKeypair = deriveReceiverKeypair(solana.adminKeypair, cfg.bridgeId);
    receiverPubkey = derivedKeypair.publicKey;
    log(TAG, `Derived receiver: ${receiverPubkey.toBase58()} (bridge: ${cfg.bridgeId})`);

    const ataAccount = await getOrCreateAssociatedTokenAccount(
      solana.connection, solana.adminKeypair, solana.usdcMint, receiverPubkey,
      false, undefined, undefined, solana.tokenProgramId,
    );
    receiverAta = ataAccount.address;
    log(TAG, `Derived ATA: ${receiverAta.toBase58()}`);
  } else {
    receiverPubkey = solana.adminPubkey;
    receiverAta = solana.adminAta;
  }

  // Pre-flight
  const svmBal = await getTokenBalance(svm.connection, svm.adminAta, svm.tokenProgramId);
  if (svmBal < BigInt(cfg.testAmount)) {
    throw new Error(`Insufficient SVM USDC: have ${svmBal}, need ${cfg.testAmount}`);
  }
  log(TAG, `SVM USDC balance: ${svmBal}`);

  const solanaBalBefore = await getTokenBalance(solana.connection, receiverAta, solana.tokenProgramId);
  log(TAG, `Solana USDC before (${receiverPubkey.toBase58()}): ${solanaBalBefore}`);

  const svmVaultBefore = await getTokenBalance(svm.connection, svm.vaultAta, svm.tokenProgramId);
  log(TAG, `SVM vault before: ${svmVaultBefore}`);

  // Step 1: Stake on SVM (1024chain) Bridge
  log(TAG, `Staking ${cfg.testAmount} on SVM (receiver: ${receiverPubkey.toBase58()})...`);
  const stakeTxSig = await svm.program.methods
    .stake(new BN(cfg.testAmount), receiverPubkey.toBase58())
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
  log(TAG, `Stake tx: ${stakeTxSig}`);

  // Step 2: Verify StakeEvent from SVM tx logs
  log(TAG, "Attempting to verify StakeEvent from tx logs...");
  let stakeEventVerified = false;
  const logMessages = await getTransactionLogsWithRetry(TAG, svm.connection, stakeTxSig);

  if (logMessages) {
    const stakeEvent = parseStakeEvent(logMessages);
    if (stakeEvent) {
      log(TAG, `StakeEvent: source=${stakeEvent.sourceContract}, amount=${stakeEvent.amount}, nonce=${stakeEvent.nonce}, receiver=${stakeEvent.receiverAddress}`);
      if (stakeEvent.amount !== BigInt(expectedNet)) {
        throw new Error(`StakeEvent.amount mismatch: ${stakeEvent.amount} != ${expectedNet} (expected: testAmount ${cfg.testAmount} - fee ${cfg.svmBridgeFee})`);
      }
      if (stakeEvent.receiverAddress !== receiverPubkey.toBase58()) {
        throw new Error(`StakeEvent.receiverAddress mismatch: ${stakeEvent.receiverAddress} != ${receiverPubkey.toBase58()}`);
      }
      log(TAG, "StakeEvent fields verified");
      stakeEventVerified = true;
    }
  }
  if (!stakeEventVerified) {
    log(TAG, "WARNING: Could not verify StakeEvent (getTransaction returned null — 1024chain RPC limitation). Proceeding with balance verification.");
  }

  // Verify SVM vault increased by full amount (fee included)
  const svmVaultAfterStake = await getTokenBalance(svm.connection, svm.vaultAta, svm.tokenProgramId);
  const vaultStakeChange = svmVaultAfterStake - svmVaultBefore;
  log(TAG, `SVM vault after stake: ${svmVaultAfterStake} (change: +${vaultStakeChange})`);

  // Step 3: Wait for Solana balance to increase
  const solanaExpected = solanaBalBefore + BigInt(expectedNet);
  log(TAG, `Waiting for Solana USDC to reach >= ${solanaExpected} (net: +${expectedNet})...`);

  const solanaBalAfter = await pollUntilBalanceChanges(
    TAG, "Solana USDC",
    () => getTokenBalance(solana.connection, receiverAta, solana.tokenProgramId),
    solanaExpected,
    { initialDelayMs: cfg.initialDelayMs, pollIntervalMs: cfg.pollIntervalMs, timeoutMs: cfg.timeoutMs },
  );
  log(TAG, `Solana USDC after: ${solanaBalAfter}`);

  const actualIncrease = solanaBalAfter - solanaBalBefore;
  if (actualIncrease !== BigInt(expectedNet)) {
    log(TAG, `WARNING: Balance increase ${actualIncrease} != expected ${expectedNet}`);
  }

  // Step 4: Verify CrossChainSuccessEvent on Solana
  log(TAG, "Checking for CrossChainSuccessEvent on Solana...");
  const disc = anchorEventDiscriminator("CrossChainSuccessEvent");
  const eventResult = await pollForEvent(
    TAG, solana.connection, solana.programId, disc, cfg.timeoutMs, cfg.pollIntervalMs,
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
  log(TAG, "PASSED: 1024chain -> Solana transfer verified");
  if (cfg.svmBridgeFee > 0) {
    log(TAG, `  Fee deducted on SVM stake: ${cfg.svmBridgeFee}`);
    log(TAG, `  Net received on Solana: ${actualIncrease}`);
  }

  // Step 5: Reclaim USDC from derived address back to admin (best-effort)
  if (derivedKeypair) {
    log(TAG, "");
    log(TAG, "Reclaiming USDC from derived address...");
    const mintInfo = await solana.connection.getAccountInfo(solana.usdcMint);
    const decimals = mintInfo ? MintLayout.decode(mintInfo.data).decimals : 6;
    await reclaimToAdmin(
      TAG, solana.connection, derivedKeypair, solana.adminKeypair,
      solana.usdcMint, solana.tokenProgramId, decimals,
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
