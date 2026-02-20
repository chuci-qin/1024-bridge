/**
 * e2e-smoke-test.ts
 *
 * End-to-end smoke test for the Bridge1024 cross-chain bridge.
 * Performs a small transfer in each direction (SVM->EVM and EVM->SVM)
 * and verifies that the tokens arrive on the destination chain.
 *
 * Required environment variables:
 *   ADMIN_KEYPAIR_PATH    - Path to admin SVM keypair JSON file
 *   ADMIN_EVM_PRIVATE_KEY - Admin EVM private key (hex, with 0x prefix)
 *   EVM_RPC_URL           - EVM RPC endpoint
 *   SVM_RPC_URL           - Solana RPC endpoint
 *   EVM_CONTRACT_ADDRESS  - Deployed Bridge1024 EVM contract address
 *   SVM_PROGRAM_ID        - Deployed Bridge1024 SVM program ID (base58)
 *   EVM_TOKEN_ADDRESS     - USDC ERC20 contract address on EVM
 *   SVM_TOKEN_ADDRESS     - USDC SPL token mint on SVM (base58)
 *   IDL_PATH              - Path to bridge1024 Anchor IDL JSON file
 *
 * Optional environment variables:
 *   TEST_AMOUNT           - Amount in smallest unit (default: 10000 = 0.01 USDC)
 *   INITIAL_DELAY_MS      - Delay after stake before first poll (default: 5000)
 *   POLL_INTERVAL_MS      - Polling interval (default: 5000)
 *   TIMEOUT_MS            - Max wait per direction (default: 60000)
 */

import * as fs from "fs";
import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import {
  TOKEN_2022_PROGRAM_ID,
  getAssociatedTokenAddress,
  getAccount,
} from "@solana/spl-token";
import { ethers } from "ethers";
import BN from "bn.js";

// ---- Environment ----

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Missing required env: ${name}`);
  return value;
}

const ADMIN_KEYPAIR_PATH = requireEnv("ADMIN_KEYPAIR_PATH");
const ADMIN_EVM_PRIVATE_KEY = requireEnv("ADMIN_EVM_PRIVATE_KEY");
const EVM_RPC_URL = requireEnv("EVM_RPC_URL");
const SVM_RPC_URL = requireEnv("SVM_RPC_URL");
const EVM_CONTRACT_ADDRESS = requireEnv("EVM_CONTRACT_ADDRESS");
const SVM_PROGRAM_ID = requireEnv("SVM_PROGRAM_ID");
const EVM_TOKEN_ADDRESS = requireEnv("EVM_TOKEN_ADDRESS");
const SVM_TOKEN_ADDRESS = requireEnv("SVM_TOKEN_ADDRESS");
const IDL_PATH = requireEnv("IDL_PATH");

const TEST_AMOUNT = parseInt(process.env.TEST_AMOUNT || "10000");
const INITIAL_DELAY_MS = parseInt(process.env.INITIAL_DELAY_MS || "5000");
const POLL_INTERVAL_MS = parseInt(process.env.POLL_INTERVAL_MS || "5000");
const TIMEOUT_MS = parseInt(process.env.TIMEOUT_MS || "60000");

// ---- Helpers ----

function loadKeypair(path: string): Keypair {
  const raw = JSON.parse(fs.readFileSync(path, "utf-8"));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function log(msg: string): void {
  console.log(`[e2e][${new Date().toISOString()}] ${msg}`);
}

const ERC20_ABI = [
  "function balanceOf(address owner) view returns (uint256)",
  "function approve(address spender, uint256 amount) returns (bool)",
];

const BRIDGE_ABI = [
  "function stake(uint256 amount, string receiverAddress) returns (uint64)",
];

// ---- Polling ----

async function pollUntilBalanceChanges(
  label: string,
  getBalance: () => Promise<bigint>,
  expectedMinimum: bigint,
): Promise<bigint> {
  log(`  Waiting ${INITIAL_DELAY_MS}ms before first poll...`);
  await sleep(INITIAL_DELAY_MS);

  const deadline = Date.now() + TIMEOUT_MS;
  while (Date.now() < deadline) {
    const current = await getBalance();
    log(`  ${label} balance: ${current}`);
    if (current >= expectedMinimum) {
      return current;
    }
    log(`  Not yet, polling again in ${POLL_INTERVAL_MS}ms...`);
    await sleep(POLL_INTERVAL_MS);
  }
  throw new Error(
    `Timeout (${TIMEOUT_MS}ms): ${label} balance did not reach ${expectedMinimum}`,
  );
}

async function getSvmTokenBalance(
  connection: Connection,
  ata: PublicKey,
  tokenProgramId: PublicKey,
): Promise<bigint> {
  try {
    const acct = await getAccount(connection, ata, "confirmed", tokenProgramId);
    return acct.amount;
  } catch {
    return 0n;
  }
}

// ---- Main ----

async function main() {
  log("============================================");
  log("  Bridge1024 - E2E Smoke Test");
  log("============================================");
  log(`EVM Contract:    ${EVM_CONTRACT_ADDRESS}`);
  log(`SVM Program:     ${SVM_PROGRAM_ID}`);
  log(`EVM Token:       ${EVM_TOKEN_ADDRESS}`);
  log(`SVM Token:       ${SVM_TOKEN_ADDRESS}`);
  log(`Test Amount:     ${TEST_AMOUNT} (smallest unit)`);
  log(`Timeout:         ${TIMEOUT_MS}ms per direction`);
  log("");

  // ---- Setup SVM ----
  const adminKeypair = loadKeypair(ADMIN_KEYPAIR_PATH);
  const adminSvmPubkey = adminKeypair.publicKey;
  log(`Admin SVM pubkey: ${adminSvmPubkey.toBase58()}`);

  const svmConnection = new Connection(SVM_RPC_URL, "confirmed");
  const svmWallet = new Wallet(adminKeypair);
  const svmProvider = new AnchorProvider(svmConnection, svmWallet, {
    commitment: "confirmed",
    preflightCommitment: "confirmed",
  });
  anchor.setProvider(svmProvider);

  const idl = JSON.parse(fs.readFileSync(IDL_PATH, "utf-8"));
  const programId = new PublicKey(SVM_PROGRAM_ID);
  if (idl.address) idl.address = SVM_PROGRAM_ID;
  if (idl.metadata?.address) idl.metadata.address = SVM_PROGRAM_ID;
  const svmProgram = new Program(idl, svmProvider);

  const usdcMint = new PublicKey(SVM_TOKEN_ADDRESS);
  const [senderState] = PublicKey.findProgramAddressSync(
    [Buffer.from("sender_state")],
    programId,
  );
  const [vault] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    programId,
  );

  const mintAccountInfo = await svmConnection.getAccountInfo(usdcMint);
  if (!mintAccountInfo) throw new Error(`USDC mint not found: ${SVM_TOKEN_ADDRESS}`);
  const tokenProgramId = mintAccountInfo.owner;
  log(`SVM Token Program: ${tokenProgramId.toBase58()} (${tokenProgramId.equals(TOKEN_2022_PROGRAM_ID) ? "Token-2022" : "SPL Token"})`);

  const adminSvmAta = await getAssociatedTokenAddress(
    usdcMint, adminSvmPubkey, false, tokenProgramId,
  );
  const vaultAta = await getAssociatedTokenAddress(
    usdcMint, vault, true, tokenProgramId,
  );

  // ---- Setup EVM ----
  const evmProvider = new ethers.JsonRpcProvider(EVM_RPC_URL);
  const evmWallet = new ethers.Wallet(ADMIN_EVM_PRIVATE_KEY, evmProvider);
  const adminEvmAddress = evmWallet.address;
  log(`Admin EVM address: ${adminEvmAddress}`);

  const evmUsdc = new ethers.Contract(EVM_TOKEN_ADDRESS, ERC20_ABI, evmWallet);
  const evmBridge = new ethers.Contract(EVM_CONTRACT_ADDRESS, BRIDGE_ABI, evmWallet);

  // ---- Pre-flight checks ----
  log("");
  log("=== Pre-flight Checks ===");

  const evmUsdcBal = await evmUsdc.balanceOf(adminEvmAddress) as bigint;
  const evmNativeBal = await evmProvider.getBalance(adminEvmAddress);
  log(`EVM USDC balance:   ${evmUsdcBal}`);
  log(`EVM native balance: ${evmNativeBal}`);

  const svmUsdcBal = await getSvmTokenBalance(svmConnection, adminSvmAta, tokenProgramId);
  const svmNativeBal = await svmConnection.getBalance(adminSvmPubkey);
  log(`SVM USDC balance:   ${svmUsdcBal}`);
  log(`SVM native balance: ${svmNativeBal} lamports`);

  if (svmUsdcBal < BigInt(TEST_AMOUNT)) {
    throw new Error(`Insufficient SVM USDC: have ${svmUsdcBal}, need ${TEST_AMOUNT}`);
  }
  if (evmUsdcBal < BigInt(TEST_AMOUNT)) {
    throw new Error(`Insufficient EVM USDC: have ${evmUsdcBal}, need ${TEST_AMOUNT}`);
  }

  // ==================================================
  // Test 1: SVM -> EVM
  // ==================================================
  log("");
  log("=== Test 1: SVM -> EVM ===");

  const evmBalBefore = await evmUsdc.balanceOf(adminEvmAddress) as bigint;
  log(`EVM USDC before: ${evmBalBefore}`);

  log(`Calling SVM stake(${TEST_AMOUNT}, "${adminEvmAddress}")...`);
  const svmStakeTx = await svmProgram.methods
    .stake(new BN(TEST_AMOUNT), adminEvmAddress)
    .accounts({
      senderState,
      user: adminSvmPubkey,
      vault,
      usdcMint,
      userTokenAccount: adminSvmAta,
      vaultTokenAccount: vaultAta,
      tokenProgram: tokenProgramId,
    })
    .signers([adminKeypair])
    .rpc();
  log(`SVM stake tx: ${svmStakeTx}`);

  const evmExpected = evmBalBefore + BigInt(TEST_AMOUNT);
  const evmBalAfter = await pollUntilBalanceChanges(
    "EVM USDC",
    async () => (await evmUsdc.balanceOf(adminEvmAddress)) as bigint,
    evmExpected,
  );
  log(`EVM USDC after:  ${evmBalAfter}`);
  log("Test 1 PASSED: SVM -> EVM transfer verified");

  // ==================================================
  // Test 2: EVM -> SVM
  // ==================================================
  log("");
  log("=== Test 2: EVM -> SVM ===");

  const svmBalBefore = await getSvmTokenBalance(svmConnection, adminSvmAta, tokenProgramId);
  log(`SVM USDC before: ${svmBalBefore}`);

  log(`Approving EVM USDC spend of ${TEST_AMOUNT}...`);
  const approveTx = await evmUsdc.approve(EVM_CONTRACT_ADDRESS, TEST_AMOUNT);
  await approveTx.wait();
  log(`Approve tx: ${approveTx.hash}`);

  log(`Calling EVM stake(${TEST_AMOUNT}, "${adminSvmPubkey.toBase58()}")...`);
  const evmStakeTx = await evmBridge.stake(TEST_AMOUNT, adminSvmPubkey.toBase58());
  await evmStakeTx.wait();
  log(`EVM stake tx: ${evmStakeTx.hash}`);

  const svmExpected = svmBalBefore + BigInt(TEST_AMOUNT);
  const svmBalAfter = await pollUntilBalanceChanges(
    "SVM USDC",
    () => getSvmTokenBalance(svmConnection, adminSvmAta, tokenProgramId),
    svmExpected,
  );
  log(`SVM USDC after:  ${svmBalAfter}`);
  log("Test 2 PASSED: EVM -> SVM transfer verified");

  // ==================================================
  // Summary
  // ==================================================
  log("");
  log("============================================");
  log("  E2E Smoke Test: ALL PASSED");
  log("============================================");
  log(`  SVM -> EVM: ${TEST_AMOUNT} transferred and verified`);
  log(`  EVM -> SVM: ${TEST_AMOUNT} transferred and verified`);
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(`[e2e] FAILED: ${err.message || err}`);
    if (err.stack) console.error(err.stack);
    process.exit(1);
  });
