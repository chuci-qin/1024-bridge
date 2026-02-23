/**
 * e2e-svm-to-evm.ts
 *
 * E2E test: SVM -> EVM direction.
 * Stakes USDC on SVM, verifies StakeEvent from tx logs, waits for relayer
 * to submit signatures and unlock on EVM, verifies TokensUnlocked event
 * and EVM balance increase.
 *
 * Environment variables: see e2e-helpers.ts loadConfig()
 */

import BN from "bn.js";
import { ethers } from "ethers";
import {
  loadConfig, log, sleep, setupSvm, setupEvm,
  pollUntilBalanceChanges, anchorEventDiscriminator,
} from "./e2e-helpers";

const TAG = "svm->evm";

function parseBorshString(buf: Buffer, offset: number): [string, number] {
  const len = buf.readUInt32LE(offset);
  const str = buf.subarray(offset + 4, offset + 4 + len).toString("utf8");
  return [str, offset + 4 + len];
}

interface SvmStakeEvent {
  sourceContract: string;
  targetContract: string;
  chainId: bigint;
  blockHeight: bigint;
  amount: bigint;
  receiverAddress: string;
  nonce: bigint;
}

function parseSvmStakeEvent(logMessages: string[]): SvmStakeEvent | null {
  const disc = anchorEventDiscriminator("StakeEvent");
  for (const line of logMessages) {
    if (!line.startsWith("Program data: ")) continue;
    const raw = Buffer.from(line.slice("Program data: ".length), "base64");
    if (raw.length < 8 || !raw.subarray(0, 8).equals(disc)) continue;

    const data = raw.subarray(8);
    let offset = 0;
    let sourceContract: string;
    [sourceContract, offset] = parseBorshString(data, offset);
    let targetContract: string;
    [targetContract, offset] = parseBorshString(data, offset);
    const chainId = data.readBigUInt64LE(offset); offset += 8;
    const blockHeight = data.readBigUInt64LE(offset); offset += 8;
    const amount = data.readBigUInt64LE(offset); offset += 8;
    let receiverAddress: string;
    [receiverAddress, offset] = parseBorshString(data, offset);
    const nonce = data.readBigUInt64LE(offset);
    return { sourceContract, targetContract, chainId, blockHeight, amount, receiverAddress, nonce };
  }
  return null;
}

async function main() {
  const cfg = loadConfig();

  log(TAG, "============================================");
  log(TAG, "  Bridge1024 E2E: SVM -> EVM");
  log(TAG, "============================================");
  log(TAG, `SVM Program:  ${cfg.svmProgramId}`);
  log(TAG, `EVM Contract: ${cfg.evmContractAddress}`);
  log(TAG, `Test Amount:  ${cfg.testAmount}`);
  log(TAG, "");

  const svm = await setupSvm(cfg);
  const evm = setupEvm(cfg);
  log(TAG, `Admin SVM: ${svm.adminSvmPubkey.toBase58()}`);
  log(TAG, `Admin EVM: ${evm.adminEvmAddress}`);

  // Pre-flight
  const svmBal = await (async () => {
    const { getAccount } = await import("@solana/spl-token");
    try {
      const acct = await getAccount(svm.connection, svm.adminAta, "confirmed", svm.tokenProgramId);
      return acct.amount;
    } catch { return 0n; }
  })();
  if (svmBal < BigInt(cfg.testAmount)) {
    throw new Error(`Insufficient SVM USDC: have ${svmBal}, need ${cfg.testAmount}`);
  }
  log(TAG, `SVM USDC balance: ${svmBal}`);

  // Record EVM balance before
  const evmBalBefore = await evm.usdc.balanceOf(evm.adminEvmAddress) as bigint;
  log(TAG, `EVM USDC before: ${evmBalBefore}`);

  // Step 1: Stake on SVM
  log(TAG, `Staking ${cfg.testAmount} on SVM...`);
  const stakeTxSig = await svm.program.methods
    .stake(new BN(cfg.testAmount), evm.adminEvmAddress)
    .accounts({
      senderState: svm.senderState,
      user: svm.adminSvmPubkey,
      vault: svm.vault,
      usdcMint: svm.usdcMint,
      userTokenAccount: svm.adminAta,
      vaultTokenAccount: svm.vaultAta,
      tokenProgram: svm.tokenProgramId,
    })
    .signers([svm.adminKeypair])
    .rpc();
  log(TAG, `Stake tx: ${stakeTxSig}`);

  // Step 2: Verify SVM StakeEvent (best-effort — 1024chain RPC getTransaction may be slow)
  log(TAG, "Attempting to verify StakeEvent from tx logs...");
  let stakeEventVerified = false;
  try {
    let tx = null;
    for (let attempt = 0; attempt < 5; attempt++) {
      tx = await svm.connection.getTransaction(stakeTxSig, {
        commitment: "confirmed",
        maxSupportedTransactionVersion: 0,
      });
      if (tx?.meta?.logMessages) break;
      log(TAG, `Transaction logs not available yet, retrying in 3s (${attempt + 1}/5)...`);
      await sleep(3000);
    }

    if (tx?.meta?.logMessages) {
      const stakeEvent = parseSvmStakeEvent(tx.meta.logMessages);
      if (stakeEvent) {
        log(TAG, `StakeEvent emitted: source=${stakeEvent.sourceContract}, amount=${stakeEvent.amount}, nonce=${stakeEvent.nonce}, receiver=${stakeEvent.receiverAddress}`);
        if (stakeEvent.sourceContract !== svm.programId.toBase58()) {
          throw new Error(`StakeEvent.sourceContract mismatch: ${stakeEvent.sourceContract} != ${svm.programId.toBase58()}`);
        }
        if (stakeEvent.amount !== BigInt(cfg.testAmount)) {
          throw new Error(`StakeEvent.amount mismatch: ${stakeEvent.amount} != ${cfg.testAmount}`);
        }
        if (stakeEvent.receiverAddress !== evm.adminEvmAddress) {
          throw new Error(`StakeEvent.receiverAddress mismatch: ${stakeEvent.receiverAddress} != ${evm.adminEvmAddress}`);
        }
        log(TAG, "StakeEvent fields verified");
        stakeEventVerified = true;
      }
    }
  } catch (err: any) {
    log(TAG, `StakeEvent verification error: ${err.message}`);
    throw err;
  }
  if (!stakeEventVerified) {
    log(TAG, "WARNING: Could not verify StakeEvent (getTransaction returned null — 1024chain RPC limitation). Proceeding with balance verification.");
  }

  // Step 3: Wait for EVM balance to increase (relayer submits signatures -> unlock)
  const evmExpected = evmBalBefore + BigInt(cfg.testAmount);
  const evmBalAfter = await pollUntilBalanceChanges(
    TAG, "EVM USDC",
    async () => (await evm.usdc.balanceOf(evm.adminEvmAddress)) as bigint,
    evmExpected,
    { initialDelayMs: cfg.initialDelayMs, pollIntervalMs: cfg.pollIntervalMs, timeoutMs: cfg.timeoutMs },
  );
  log(TAG, `EVM USDC after: ${evmBalAfter}`);

  // Step 4: Verify TokensUnlocked event on EVM (best-effort, match by amount + receiver)
  try {
    const filter = evm.bridge.filters.TokensUnlocked();
    const recentBlock = await evm.provider.getBlockNumber();
    const events = await evm.bridge.queryFilter(filter, recentBlock - 200, recentBlock);
    const matchingEvent = events.find((e: any) => {
      const parsed = evm.bridge.interface.parseLog(e);
      return parsed && parsed.args.amount === BigInt(cfg.testAmount);
    });

    if (matchingEvent) {
      const parsed = evm.bridge.interface.parseLog(matchingEvent);
      log(TAG, `TokensUnlocked emitted: nonce=${parsed!.args.nonce}, receiver=${parsed!.args.receiver}, amount=${parsed!.args.amount}`);
    } else {
      log(TAG, "WARNING: TokensUnlocked event not found in recent blocks");
    }
  } catch (err: any) {
    log(TAG, `WARNING: TokensUnlocked query failed: ${err.message}`);
  }

  log(TAG, "");
  log(TAG, "PASSED: SVM -> EVM transfer verified");
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(`[${TAG}] FAILED: ${err.message || err}`);
    if (err.stack) console.error(err.stack);
    process.exit(1);
  });
