import * as fs from "fs";
import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import {
  TOKEN_2022_PROGRAM_ID,
  getAssociatedTokenAddress,
  getOrCreateAssociatedTokenAccount,
  getAccount,
  createTransferCheckedInstruction,
} from "@solana/spl-token";
import {
  SystemProgram,
  Transaction,
  sendAndConfirmTransaction,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import { ethers } from "ethers";
import * as crypto from "crypto";

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

export const ERC20_ABI = [
  "function balanceOf(address owner) view returns (uint256)",
  "function approve(address spender, uint256 amount) returns (bool)",
];

export const BRIDGE_EVM_ABI = [
  "function stake(uint256 amount, string receiverAddress) returns (uint64)",
  "event StakeEvent(bytes32 indexed sourceContract, bytes32 indexed targetContract, uint64 chainId, uint64 blockHeight, uint64 amount, address sender, string receiverAddress, uint64 nonce)",
  "event TokensUnlocked(uint64 indexed nonce, address receiver, uint64 amount)",
];

export async function getSvmTokenBalance(
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

export interface E2EConfig {
  adminKeypairPath: string;
  adminEvmPrivateKey: string;
  evmRpcUrl: string;
  svmRpcUrl: string;
  evmContractAddress: string;
  svmProgramId: string;
  evmTokenAddress: string;
  svmTokenAddress: string;
  idlPath: string;
  testAmount: number;
  initialDelayMs: number;
  pollIntervalMs: number;
  timeoutMs: number;
  bridgeId?: string;
}

export function loadConfig(): E2EConfig {
  return {
    adminKeypairPath: requireEnv("ADMIN_KEYPAIR_PATH"),
    adminEvmPrivateKey: requireEnv("ADMIN_EVM_PRIVATE_KEY"),
    evmRpcUrl: requireEnv("EVM_RPC_URL"),
    svmRpcUrl: requireEnv("SVM_RPC_URL"),
    evmContractAddress: requireEnv("EVM_CONTRACT_ADDRESS"),
    svmProgramId: requireEnv("SVM_PROGRAM_ID"),
    evmTokenAddress: requireEnv("EVM_TOKEN_ADDRESS"),
    svmTokenAddress: requireEnv("SVM_TOKEN_ADDRESS"),
    idlPath: requireEnv("IDL_PATH"),
    testAmount: parseInt(process.env.TEST_AMOUNT || "10000"),
    initialDelayMs: parseInt(process.env.INITIAL_DELAY_MS || "5000"),
    pollIntervalMs: parseInt(process.env.POLL_INTERVAL_MS || "5000"),
    timeoutMs: parseInt(process.env.TIMEOUT_MS || "60000"),
    bridgeId: process.env.BRIDGE_ID || undefined,
  };
}

/**
 * Derive a deterministic SVM keypair from admin secret key + bridge ID.
 * Used to give each bridge its own isolated receiver address in E2E tests.
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

    const balance = await getSvmTokenBalance(connection, derivedAta, tokenProgramId);
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

/**
 * Derive a deterministic EVM wallet from admin EVM private key + bridge ID.
 * Used to give each bridge its own isolated receiver address for SVM->EVM tests.
 */
export function deriveEvmReceiver(adminEvmPrivateKey: string, bridgeId: string): ethers.Wallet {
  const keyBytes = Buffer.from(adminEvmPrivateKey.replace("0x", ""), "hex");
  const combined = Buffer.concat([keyBytes, Buffer.from(bridgeId, "utf-8")]);
  const hash = crypto.createHash("sha256").update(combined).digest();
  return new ethers.Wallet("0x" + hash.toString("hex"));
}

const DEFAULT_EVM_RECLAIM_THRESHOLD = 500_000n; // 0.5 USDC (6 decimals), ~50 tests

/**
 * Reclaim EVM USDC from a derived address back to admin, only when balance
 * exceeds a threshold. ETH is expensive on testnets, so we batch reclaims.
 * Best-effort: failures are logged but do not throw.
 */
export async function reclaimEvmToAdmin(
  tag: string,
  provider: ethers.JsonRpcProvider,
  derivedWallet: ethers.Wallet,
  adminWallet: ethers.Wallet,
  usdcAddress: string,
): Promise<void> {
  const threshold = BigInt(process.env.EVM_RECLAIM_THRESHOLD || DEFAULT_EVM_RECLAIM_THRESHOLD.toString());

  try {
    const usdc = new ethers.Contract(usdcAddress, [
      "function balanceOf(address) view returns (uint256)",
      "function transfer(address,uint256) returns (bool)",
    ], derivedWallet.connect(provider));

    const balance = await usdc.balanceOf(derivedWallet.address) as bigint;
    if (balance < threshold) {
      log(tag, `EVM reclaim: derived balance ${balance} < threshold ${threshold}, skipping`);
      return;
    }
    log(tag, `EVM reclaim: derived balance ${balance} >= threshold ${threshold}, reclaiming...`);

    const MIN_ETH = ethers.parseEther("0.0002");
    const ethBalance = await provider.getBalance(derivedWallet.address);
    if (ethBalance < MIN_ETH) {
      const fundAmount = ethers.parseEther("0.0005");
      log(tag, `EVM reclaim: funding ${ethers.formatEther(fundAmount)} ETH to derived address...`);
      const fundTx = await adminWallet.connect(provider).sendTransaction({
        to: derivedWallet.address,
        value: fundAmount,
      });
      await fundTx.wait();
    }

    const tx = await usdc.transfer(adminWallet.address, balance);
    await tx.wait();
    log(tag, `EVM reclaim: transferred ${balance} USDC back to admin (tx: ${tx.hash})`);
  } catch (err: any) {
    log(tag, `EVM reclaim WARNING: failed to reclaim USDC: ${err.message}`);
  }
}

export interface SvmSetup {
  adminKeypair: Keypair;
  adminSvmPubkey: PublicKey;
  connection: Connection;
  provider: AnchorProvider;
  program: Program;
  programId: PublicKey;
  usdcMint: PublicKey;
  tokenProgramId: PublicKey;
  senderState: PublicKey;
  vault: PublicKey;
  adminAta: PublicKey;
  vaultAta: PublicKey;
}

export async function setupSvm(cfg: E2EConfig): Promise<SvmSetup> {
  const adminKeypair = loadKeypair(cfg.adminKeypairPath);
  const adminSvmPubkey = adminKeypair.publicKey;

  const connection = new Connection(cfg.svmRpcUrl, "confirmed");
  const wallet = new Wallet(adminKeypair);
  const provider = new AnchorProvider(connection, wallet, {
    commitment: "confirmed",
    preflightCommitment: "confirmed",
  });
  anchor.setProvider(provider);

  const idl = JSON.parse(fs.readFileSync(cfg.idlPath, "utf-8"));
  const programId = new PublicKey(cfg.svmProgramId);
  if (idl.address) idl.address = cfg.svmProgramId;
  if (idl.metadata?.address) idl.metadata.address = cfg.svmProgramId;
  const program = new Program(idl, provider);

  const usdcMint = new PublicKey(cfg.svmTokenAddress);
  const [senderState] = PublicKey.findProgramAddressSync(
    [Buffer.from("sender_state")],
    programId,
  );
  const [vault] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    programId,
  );

  const mintAccountInfo = await connection.getAccountInfo(usdcMint);
  if (!mintAccountInfo) throw new Error(`USDC mint not found: ${cfg.svmTokenAddress}`);
  const tokenProgramId = mintAccountInfo.owner;

  const adminAta = await getAssociatedTokenAddress(usdcMint, adminSvmPubkey, false, tokenProgramId);
  const vaultAta = await getAssociatedTokenAddress(usdcMint, vault, true, tokenProgramId);

  return {
    adminKeypair, adminSvmPubkey, connection, provider, program, programId,
    usdcMint, tokenProgramId, senderState, vault, adminAta, vaultAta,
  };
}

export interface EvmSetup {
  provider: ethers.JsonRpcProvider;
  wallet: ethers.Wallet;
  adminEvmAddress: string;
  usdc: ethers.Contract;
  bridge: ethers.Contract;
}

export function setupEvm(cfg: E2EConfig): EvmSetup {
  const provider = new ethers.JsonRpcProvider(cfg.evmRpcUrl);
  const wallet = new ethers.Wallet(cfg.adminEvmPrivateKey, provider);
  const adminEvmAddress = wallet.address;

  const usdc = new ethers.Contract(cfg.evmTokenAddress, ERC20_ABI, wallet);
  const bridge = new ethers.Contract(cfg.evmContractAddress, BRIDGE_EVM_ABI, wallet);

  return { provider, wallet, adminEvmAddress, usdc, bridge };
}

export async function pollSvmEvent(
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
    const sigs = await connection.getSignaturesForAddress(programId, { limit: 10 }, "confirmed");
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

export function anchorEventDiscriminator(eventName: string): Buffer {
  const crypto = require("crypto");
  return crypto.createHash("sha256").update(`event:${eventName}`).digest().subarray(0, 8);
}

function parseBorshString(buf: Buffer, offset: number): [string, number] {
  const len = buf.readUInt32LE(offset);
  const str = buf.subarray(offset + 4, offset + 4 + len).toString("utf8");
  return [str, offset + 4 + len];
}

export interface CrossChainSuccessEventData {
  evmAddress: string;
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
    let evmAddress: string;
    [evmAddress, offset] = parseBorshString(data, offset);
    const amount = data.readBigUInt64LE(offset); offset += 8;
    const nonce = data.readBigUInt64LE(offset); offset += 8;
    const sourceChainId = data.readBigUInt64LE(offset); offset += 8;
    const blockHeight = data.readBigUInt64LE(offset); offset += 8;
    let receiverAddress: string;
    [receiverAddress, offset] = parseBorshString(data, offset);
    return { evmAddress, amount, nonce, sourceChainId, blockHeight, receiverAddress };
  }
  return null;
}
