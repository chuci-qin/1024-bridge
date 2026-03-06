/**
 * e2e-helpers.ts
 *
 * Shared utilities for Sol-1024 bridge E2E tests.
 * Provides config loading, chain setup, balance polling, event parsing,
 * and BRIDGE_ID-based receiver isolation for concurrent CI/CD tests.
 *
 * Both chains are Solana-compatible (Solana + 1024chain/SVM), so we use
 * Anchor + @solana/web3.js for both sides.
 */

import * as fs from "fs";
import * as crypto from "crypto";
import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import {
  getAssociatedTokenAddress,
  getOrCreateAssociatedTokenAccount,
  getAccount,
  createTransferCheckedInstruction,
} from "@solana/spl-token";
import {
  SystemProgram,
  Transaction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";

// ============================================================
// Basic Utilities
// ============================================================

export function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Missing required env: ${name}`);
  return value;
}

export function loadKeypair(path: string): Keypair {
  const raw = JSON.parse(fs.readFileSync(path, "utf-8"));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function log(tag: string, msg: string): void {
  console.log(`[${tag}][${new Date().toISOString()}] ${msg}`);
}

// ============================================================
// Configuration
// ============================================================

export interface E2EConfig {
  solanaKeypairPath: string;
  svmKeypairPath: string;
  solanaRpcUrl: string;
  svmRpcUrl: string;
  solanaProgramId: string;
  svmProgramId: string;
  solanaTokenAddress: string;
  svmTokenAddress: string;
  solanaIdlPath: string;
  svmIdlPath: string;
  testAmount: number;
  svmBridgeFee: number;
  initialDelayMs: number;
  pollIntervalMs: number;
  timeoutMs: number;
  bridgeId?: string;
}

export function loadConfig(): E2EConfig {
  return {
    solanaKeypairPath: requireEnv("SOLANA_KEYPAIR_PATH"),
    svmKeypairPath: requireEnv("SVM_KEYPAIR_PATH"),
    solanaRpcUrl: requireEnv("SOLANA_RPC_URL"),
    svmRpcUrl: requireEnv("SVM_RPC_URL"),
    solanaProgramId: requireEnv("SOLANA_PROGRAM_ID"),
    svmProgramId: requireEnv("SVM_PROGRAM_ID"),
    solanaTokenAddress: requireEnv("SOLANA_TOKEN_ADDRESS"),
    svmTokenAddress: requireEnv("SVM_TOKEN_ADDRESS"),
    solanaIdlPath: requireEnv("SOLANA_IDL_PATH"),
    svmIdlPath: requireEnv("SVM_IDL_PATH"),
    testAmount: parseInt(process.env.TEST_AMOUNT || "10000"),
    svmBridgeFee: parseInt(process.env.SVM_BRIDGE_FEE || "0"),
    initialDelayMs: parseInt(process.env.INITIAL_DELAY_MS || "5000"),
    pollIntervalMs: parseInt(process.env.POLL_INTERVAL_MS || "5000"),
    timeoutMs: parseInt(process.env.TIMEOUT_MS || "60000"),
    bridgeId: process.env.BRIDGE_ID || undefined,
  };
}

// ============================================================
// Chain Setup
// ============================================================

export interface ChainSetup {
  adminKeypair: Keypair;
  adminPubkey: PublicKey;
  connection: Connection;
  provider: AnchorProvider;
  program: Program;
  programId: PublicKey;
  usdcMint: PublicKey;
  tokenProgramId: PublicKey;
  senderState: PublicKey;
  receiverState: PublicKey;
  vault: PublicKey;
  adminAta: PublicKey;
  vaultAta: PublicKey;
}

async function setupChainInternal(
  rpcUrl: string,
  keypairPath: string,
  programIdStr: string,
  tokenAddressStr: string,
  idlPath: string,
): Promise<ChainSetup> {
  const adminKeypair = loadKeypair(keypairPath);
  const adminPubkey = adminKeypair.publicKey;
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
  const mintAccountInfo = await connection.getAccountInfo(usdcMint);
  if (!mintAccountInfo) throw new Error(`USDC mint not found: ${tokenAddressStr}`);
  const tokenProgramId = mintAccountInfo.owner;

  const [senderState] = PublicKey.findProgramAddressSync(
    [Buffer.from("sender_state")],
    programId,
  );
  const [receiverState] = PublicKey.findProgramAddressSync(
    [Buffer.from("receiver_state")],
    programId,
  );
  const [vault] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    programId,
  );

  const adminAta = await getAssociatedTokenAddress(usdcMint, adminPubkey, false, tokenProgramId);
  const vaultAta = await getAssociatedTokenAddress(usdcMint, vault, true, tokenProgramId);

  return {
    adminKeypair, adminPubkey, connection, provider, program, programId,
    usdcMint, tokenProgramId, senderState, receiverState, vault, adminAta, vaultAta,
  };
}

export async function setupSolana(cfg: E2EConfig): Promise<ChainSetup> {
  return setupChainInternal(
    cfg.solanaRpcUrl,
    cfg.solanaKeypairPath,
    cfg.solanaProgramId,
    cfg.solanaTokenAddress,
    cfg.solanaIdlPath,
  );
}

export async function setupSvm(cfg: E2EConfig): Promise<ChainSetup> {
  return setupChainInternal(
    cfg.svmRpcUrl,
    cfg.svmKeypairPath,
    cfg.svmProgramId,
    cfg.svmTokenAddress,
    cfg.svmIdlPath,
  );
}

// ============================================================
// Token Balance
// ============================================================

export async function getTokenBalance(
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

export async function pollUntilBalanceChanges(
  tag: string,
  label: string,
  getBalance: () => Promise<bigint>,
  expectedMinimum: bigint,
  opts: { initialDelayMs: number; pollIntervalMs: number; timeoutMs: number },
): Promise<bigint> {
  log(tag, `Waiting ${opts.initialDelayMs}ms before first poll...`);
  await sleep(opts.initialDelayMs);

  const deadline = Date.now() + opts.timeoutMs;
  while (Date.now() < deadline) {
    const current = await getBalance();
    log(tag, `${label} balance: ${current}`);
    if (current >= expectedMinimum) {
      return current;
    }
    log(tag, `Not yet, polling again in ${opts.pollIntervalMs}ms...`);
    await sleep(opts.pollIntervalMs);
  }
  throw new Error(
    `Timeout (${opts.timeoutMs}ms): ${label} balance did not reach ${expectedMinimum}`,
  );
}

// ============================================================
// Event Parsing
// ============================================================

export function anchorEventDiscriminator(eventName: string): Buffer {
  return crypto.createHash("sha256").update(`event:${eventName}`).digest().subarray(0, 8);
}

function parseBorshString(buf: Buffer, offset: number): [string, number] {
  const len = buf.readUInt32LE(offset);
  const str = buf.subarray(offset + 4, offset + 4 + len).toString("utf8");
  return [str, offset + 4 + len];
}

export interface StakeEventData {
  sourceContract: string;
  targetContract: string;
  chainId: bigint;
  blockHeight: bigint;
  amount: bigint;
  receiverAddress: string;
  nonce: bigint;
}

/**
 * Parse a StakeEvent from Anchor program logs (unified contract format).
 * Field order: source_contract, target_contract, chain_id, block_height,
 *              amount, receiver_address, nonce.
 * No `sender` field — sender is the transaction signer.
 */
export function parseStakeEvent(logMessages: string[]): StakeEventData | null {
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

export interface CrossChainSuccessEventData {
  senderAddress: string;
  amount: bigint;
  nonce: bigint;
  sourceChainId: bigint;
  blockHeight: bigint;
  receiverAddress: string;
}

export function parseCrossChainSuccessEvent(logMessages: string[]): CrossChainSuccessEventData | null {
  const disc = anchorEventDiscriminator("CrossChainSuccessEvent");
  for (const line of logMessages) {
    if (!line.startsWith("Program data: ")) continue;
    const raw = Buffer.from(line.slice("Program data: ".length), "base64");
    if (raw.length < 8 || !raw.subarray(0, 8).equals(disc)) continue;

    const data = raw.subarray(8);
    let offset = 0;
    let senderAddress: string;
    [senderAddress, offset] = parseBorshString(data, offset);
    const amount = data.readBigUInt64LE(offset); offset += 8;
    const nonce = data.readBigUInt64LE(offset); offset += 8;
    const sourceChainId = data.readBigUInt64LE(offset); offset += 8;
    const blockHeight = data.readBigUInt64LE(offset); offset += 8;
    let receiverAddress: string;
    [receiverAddress, offset] = parseBorshString(data, offset);
    return { senderAddress, amount, nonce, sourceChainId, blockHeight, receiverAddress };
  }
  return null;
}

export async function pollForEvent(
  tag: string,
  connection: Connection,
  programId: PublicKey,
  eventDiscriminator: Buffer,
  timeoutMs: number,
  pollIntervalMs: number,
): Promise<{ logMessages: string[]; signature: string } | null> {
  const deadline = Date.now() + timeoutMs;
  const seen = new Set<string>();

  while (Date.now() < deadline) {
    const sigs = await connection.getSignaturesForAddress(programId, { limit: 20 }, "confirmed");
    for (const sig of sigs) {
      if (sig.err || seen.has(sig.signature)) continue;
      seen.add(sig.signature);

      const tx = await connection.getTransaction(sig.signature, {
        commitment: "confirmed",
        maxSupportedTransactionVersion: 0,
      });
      if (!tx?.meta?.logMessages) continue;

      for (const logLine of tx.meta.logMessages) {
        if (!logLine.startsWith("Program data: ")) continue;
        const raw = Buffer.from(logLine.slice("Program data: ".length), "base64");
        if (raw.length >= 8 && raw.subarray(0, 8).equals(eventDiscriminator)) {
          log(tag, `Found target event in tx ${sig.signature}`);
          return { logMessages: tx.meta.logMessages, signature: sig.signature };
        }
      }
    }
    log(tag, `Event not found yet, polling again in ${pollIntervalMs}ms...`);
    await sleep(pollIntervalMs);
  }
  return null;
}

/**
 * Fetch transaction logs with retry. Useful for chains where RPC may return
 * null for recently confirmed transactions.
 */
export async function getTransactionLogsWithRetry(
  tag: string,
  connection: Connection,
  txSig: string,
  maxRetries: number = 5,
  delayMs: number = 3000,
): Promise<string[] | null> {
  for (let attempt = 0; attempt < maxRetries; attempt++) {
    const tx = await connection.getTransaction(txSig, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    if (tx?.meta?.logMessages) return tx.meta.logMessages;
    log(tag, `Transaction logs not available yet, retrying in ${delayMs / 1000}s (${attempt + 1}/${maxRetries})...`);
    await sleep(delayMs);
  }
  return null;
}

// ============================================================
// Test User Setup
// ============================================================

/**
 * Create a fresh user keypair, airdrop SOL, create ATA, and fund with USDC.
 * The admin transfers USDC from their own ATA to the new user.
 */
export async function createFundedUser(
  tag: string,
  connection: Connection,
  adminKeypair: Keypair,
  usdcMint: PublicKey,
  tokenProgramId: PublicKey,
  usdcAmount: number,
  decimals: number = 6,
): Promise<{ keypair: Keypair; ata: PublicKey }> {
  const userKeypair = Keypair.generate();
  log(tag, `Created test user: ${userKeypair.publicKey.toBase58()}`);

  const airdropSig = await connection.requestAirdrop(userKeypair.publicKey, 2_000_000_000);
  await connection.confirmTransaction(airdropSig, "confirmed");
  log(tag, `Airdropped 2 SOL to test user`);

  const ataAccount = await getOrCreateAssociatedTokenAccount(
    connection, adminKeypair, usdcMint, userKeypair.publicKey,
    false, undefined, undefined, tokenProgramId,
  );
  log(tag, `Created user ATA: ${ataAccount.address.toBase58()}`);

  const adminAta = await getAssociatedTokenAddress(usdcMint, adminKeypair.publicKey, false, tokenProgramId);
  const transferIx = createTransferCheckedInstruction(
    adminAta, usdcMint, ataAccount.address, adminKeypair.publicKey,
    BigInt(usdcAmount), decimals, [], tokenProgramId,
  );
  const tx = new Transaction().add(transferIx);
  await sendAndConfirmTransaction(connection, tx, [adminKeypair]);
  log(tag, `Funded test user with ${usdcAmount} USDC`);

  return { keypair: userKeypair, ata: ataAccount.address };
}

/**
 * Create an Anchor Program instance using a user's keypair as wallet/signer.
 */
export function createUserProgram(
  connection: Connection,
  userKeypair: Keypair,
  programIdStr: string,
  idlPath: string,
): Program {
  const wallet = new Wallet(userKeypair);
  const provider = new AnchorProvider(connection, wallet, {
    commitment: "confirmed",
    preflightCommitment: "confirmed",
  });
  const idl = JSON.parse(fs.readFileSync(idlPath, "utf-8"));
  const programId = new PublicKey(programIdStr);
  if (idl.address) idl.address = programIdStr;
  if (idl.metadata?.address) idl.metadata.address = programIdStr;
  return new Program(idl, provider);
}

// ============================================================
// BRIDGE_ID Receiver Isolation
// ============================================================

/**
 * Derive a deterministic keypair from admin secret key + bridge ID.
 * Used to give each bridge its own isolated receiver address in E2E tests,
 * preventing concurrent CI/CD tests from interfering with each other's
 * balance checks.
 */
export function deriveReceiverKeypair(adminKeypair: Keypair, bridgeId: string): Keypair {
  const adminSeed = adminKeypair.secretKey.slice(0, 32);
  const combined = Buffer.concat([adminSeed, Buffer.from(bridgeId, "utf-8")]);
  const hash = crypto.createHash("sha256").update(combined).digest();
  return Keypair.fromSeed(Uint8Array.from(hash));
}

/**
 * Reclaim all USDC from a derived address back to the admin wallet.
 * Best-effort: failures are logged but do not throw.
 */
export async function reclaimToAdmin(
  tag: string,
  connection: Connection,
  derivedKeypair: Keypair,
  adminKeypair: Keypair,
  usdcMint: PublicKey,
  tokenProgramId: PublicKey,
  decimals: number,
): Promise<void> {
  try {
    const derivedAta = await getAssociatedTokenAddress(
      usdcMint, derivedKeypair.publicKey, false, tokenProgramId,
    );
    const adminAta = await getAssociatedTokenAddress(
      usdcMint, adminKeypair.publicKey, false, tokenProgramId,
    );

    const balance = await getTokenBalance(connection, derivedAta, tokenProgramId);
    if (balance <= 0n) {
      log(tag, "Reclaim: derived address has 0 USDC, nothing to reclaim");
      return;
    }
    log(tag, `Reclaim: derived address has ${balance} USDC, transferring to admin...`);

    const MIN_SOL_FOR_TX = 10_000;
    const solBalance = await connection.getBalance(derivedKeypair.publicKey);
    if (solBalance < MIN_SOL_FOR_TX) {
      const fundAmount = 100_000;
      log(tag, `Reclaim: derived SOL balance ${solBalance} < ${MIN_SOL_FOR_TX}, funding ${fundAmount} lamports...`);
      const fundTx = new Transaction().add(
        SystemProgram.transfer({
          fromPubkey: adminKeypair.publicKey,
          toPubkey: derivedKeypair.publicKey,
          lamports: fundAmount,
        }),
      );
      await sendAndConfirmTransaction(connection, fundTx, [adminKeypair]);
    }

    const transferIx = createTransferCheckedInstruction(
      derivedAta,
      usdcMint,
      adminAta,
      derivedKeypair.publicKey,
      BigInt(balance.toString()),
      decimals,
      [],
      tokenProgramId,
    );
    const tx = new Transaction().add(transferIx);
    const sig = await sendAndConfirmTransaction(connection, tx, [derivedKeypair]);
    log(tag, `Reclaim: transferred ${balance} USDC back to admin (tx: ${sig})`);
  } catch (err: any) {
    log(tag, `Reclaim WARNING: failed to reclaim USDC: ${err.message}`);
  }
}
