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
  };
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
