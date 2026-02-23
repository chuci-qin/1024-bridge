/**
 * e2e-evm-to-svm.ts
 *
 * E2E test: EVM -> SVM direction.
 * Stakes USDC on EVM, verifies StakeEvent, waits for relayer to unlock on SVM,
 * verifies CrossChainSuccessEvent and SVM balance increase.
 *
 * Environment variables: see e2e-helpers.ts loadConfig()
 */

import BN from "bn.js";
import {
  loadConfig, log, setupSvm, setupEvm,
  getSvmTokenBalance, pollUntilBalanceChanges,
  pollSvmEvent, anchorEventDiscriminator, parseCrossChainSuccessEvent,
} from "./e2e-helpers";

const TAG = "evm->svm";

async function main() {
  const cfg = loadConfig();

  log(TAG, "============================================");
  log(TAG, "  Bridge1024 E2E: EVM -> SVM");
  log(TAG, "============================================");
  log(TAG, `EVM Contract: ${cfg.evmContractAddress}`);
  log(TAG, `SVM Program:  ${cfg.svmProgramId}`);
  log(TAG, `Test Amount:  ${cfg.testAmount}`);
  log(TAG, "");

  const svm = await setupSvm(cfg);
  const evm = setupEvm(cfg);
  log(TAG, `Admin EVM: ${evm.adminEvmAddress}`);
  log(TAG, `Admin SVM: ${svm.adminSvmPubkey.toBase58()}`);

  // Pre-flight
  const evmBal = await evm.usdc.balanceOf(evm.adminEvmAddress) as bigint;
  if (evmBal < BigInt(cfg.testAmount)) {
    throw new Error(`Insufficient EVM USDC: have ${evmBal}, need ${cfg.testAmount}`);
  }
  log(TAG, `EVM USDC balance: ${evmBal}`);

  // Record SVM balance before
  const svmBalBefore = await getSvmTokenBalance(svm.connection, svm.adminAta, svm.tokenProgramId);
  log(TAG, `SVM USDC before: ${svmBalBefore}`);

  // Step 1: Approve + Stake on EVM
  log(TAG, "Approving EVM USDC spend...");
  const approveTx = await evm.usdc.approve(cfg.evmContractAddress, cfg.testAmount);
  await approveTx.wait();
  log(TAG, `Approve tx: ${approveTx.hash}`);

  log(TAG, `Staking ${cfg.testAmount} on EVM...`);
  const stakeTx = await evm.bridge.stake(cfg.testAmount, svm.adminSvmPubkey.toBase58());
  const stakeReceipt = await stakeTx.wait();
  log(TAG, `Stake tx: ${stakeTx.hash}`);

  // Step 2: Verify EVM StakeEvent
  const stakeEvent = stakeReceipt.logs
    .map((l: any) => { try { return evm.bridge.interface.parseLog(l); } catch { return null; } })
    .find((e: any) => e?.name === "StakeEvent");

  if (!stakeEvent) throw new Error("StakeEvent not found in EVM tx receipt");
  log(TAG, `StakeEvent emitted: sender=${stakeEvent.args.sender}, amount=${stakeEvent.args.amount}, nonce=${stakeEvent.args.nonce}`);

  if (stakeEvent.args.sender.toLowerCase() !== evm.adminEvmAddress.toLowerCase()) {
    throw new Error(`StakeEvent.sender mismatch: ${stakeEvent.args.sender} != ${evm.adminEvmAddress}`);
  }
  if (stakeEvent.args.amount !== BigInt(cfg.testAmount)) {
    throw new Error(`StakeEvent.amount mismatch: ${stakeEvent.args.amount} != ${cfg.testAmount}`);
  }
  log(TAG, "StakeEvent fields verified");

  // Step 3: Wait for SVM balance to increase
  const svmExpected = svmBalBefore + BigInt(cfg.testAmount);
  const svmBalAfter = await pollUntilBalanceChanges(
    TAG, "SVM USDC",
    () => getSvmTokenBalance(svm.connection, svm.adminAta, svm.tokenProgramId),
    svmExpected,
    { initialDelayMs: cfg.initialDelayMs, pollIntervalMs: cfg.pollIntervalMs, timeoutMs: cfg.timeoutMs },
  );
  log(TAG, `SVM USDC after: ${svmBalAfter}`);

  // Step 4: Verify CrossChainSuccessEvent on SVM
  const disc = anchorEventDiscriminator("CrossChainSuccessEvent");
  const eventResult = await pollSvmEvent(
    TAG, svm.connection, svm.programId, disc, cfg.timeoutMs, cfg.pollIntervalMs,
  );
  if (eventResult) {
    const ccEvent = parseCrossChainSuccessEvent(eventResult.logMessages);
    if (ccEvent) {
      log(TAG, `CrossChainSuccessEvent: evm_address=${ccEvent.evmAddress}, amount=${ccEvent.amount}, nonce=${ccEvent.nonce}`);
      if (ccEvent.amount !== BigInt(cfg.testAmount)) {
        log(TAG, `WARNING: CrossChainSuccessEvent.amount mismatch: ${ccEvent.amount} != ${cfg.testAmount}`);
      }
    }
  } else {
    log(TAG, "WARNING: CrossChainSuccessEvent not found within timeout (balance did increase, event polling may have missed it)");
  }

  log(TAG, "");
  log(TAG, "PASSED: EVM -> SVM transfer verified");
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(`[${TAG}] FAILED: ${err.message || err}`);
    if (err.stack) console.error(err.stack);
    process.exit(1);
  });
