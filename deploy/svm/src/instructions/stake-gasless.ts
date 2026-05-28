// stake-gasless.ts — User-side `stake_gasless` for the leaf SVM bridge.
//
// Differences vs stake.ts:
//   - Calls `.stakeGasless(...)` instead of `.stake(...)`.
//   - On top of bridgeFee, the contract also deducts gaslessFee from the
//     user's USDC. Both fees stay in the vault.
//   - Off-chain expectation: a paymaster service signs the Solana tx as
//     `fee_payer` (the user only signs as USDC authority). For local /
//     self-test use the same keypair as fee_payer + authority.
//   - Leaf-only: hub program has no `stake_gasless` instruction.
//
// CLI:
//   --rpc-url, --keypair, --program-id, --program-kind leaf
//   --amount   <u64 raw USDC>
//   --receiver <hex64 | base58>
//   [--fee-payer-keypair <path>]   optional: separate paymaster keypair
//                                  (defaults to --keypair = self-funded)

import {
  Keypair,
  PublicKey,
  Signer,
  SystemProgram,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAssociatedTokenAddressSync,
  createAssociatedTokenAccountInstruction,
} from "@solana/spl-token";
import * as anchor from "@coral-xyz/anchor";
import * as fs from "fs";
import {
  createClient,
  getBridgeStatePda,
  getVaultPda,
  parseArgs,
} from "../client";
import { randomBytes } from "crypto";

function randomNonce(): anchor.BN {
  for (let i = 0; i < 4; i++) {
    const buf = randomBytes(8);
    const v = buf.readBigUInt64LE(0);
    if (v !== 0n) return new anchor.BN(v.toString());
  }
  return new anchor.BN(1);
}

function decodeReceiver(input: string): number[] {
  const raw = input.trim();
  const hex = raw.startsWith("0x") || raw.startsWith("0X") ? raw.slice(2) : raw;
  if (/^[0-9a-fA-F]+$/.test(hex) && hex.length === 64) {
    return Array.from(Buffer.from(hex, "hex"));
  }
  const pk = new PublicKey(raw);
  return Array.from(pk.toBytes());
}

function loadKeypairFile(path: string): Keypair {
  const raw = JSON.parse(fs.readFileSync(path, "utf-8"));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}

async function main() {
  const baseConfig = parseArgs();
  if (baseConfig.programKind !== "leaf") {
    throw new Error("stake_gasless is a leaf-only instruction.");
  }

  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const amountStr = extra["amount"];
  const receiverStr = extra["receiver"];
  if (!amountStr || !receiverStr) {
    throw new Error("Missing required args: --amount, --receiver");
  }
  const amount = new anchor.BN(amountStr);
  const receiver = decodeReceiver(receiverStr);

  const { program, programId, connection, keypair } = createClient(baseConfig);
  // `keypair` from createClient is the user (USDC authority); paymaster may be
  // a separate signer. When not supplied, fee-payer == user (self-funded).
  const feePayer: Signer = extra["fee-payer-keypair"]
    ? loadKeypairFile(extra["fee-payer-keypair"])
    : keypair;

  const bridgeState = getBridgeStatePda(programId);
  const vault = getVaultPda(programId);

  const bs: any = await (program.account as any).bridgeState.fetch(bridgeState);
  if (bs.usdcMint.equals(PublicKey.default)) {
    throw new Error("USDC mint not configured — run 'Configure' first.");
  }
  if (bs.isPaused) {
    throw new Error("Bridge is paused — cannot stake.");
  }
  if (bs.gaslessFee.isZero()) {
    throw new Error("gasless_fee == 0 → gasless path disabled (GaslessDisabled).");
  }

  const usdcMint: PublicKey = bs.usdcMint;
  const mintAccountInfo = await connection.getAccountInfo(usdcMint);
  if (!mintAccountInfo) {
    throw new Error(`USDC mint ${usdcMint.toBase58()} not found on-chain.`);
  }
  const tokenProgram = mintAccountInfo.owner.equals(TOKEN_2022_PROGRAM_ID)
    ? TOKEN_2022_PROGRAM_ID
    : TOKEN_PROGRAM_ID;

  const userTokenAccount = getAssociatedTokenAddressSync(
    usdcMint,
    keypair.publicKey,
    false,
    tokenProgram,
    ASSOCIATED_TOKEN_PROGRAM_ID,
  );
  const vaultTokenAccount = getAssociatedTokenAddressSync(
    usdcMint,
    vault,
    true,
    tokenProgram,
    ASSOCIATED_TOKEN_PROGRAM_ID,
  );

  const userInfo = await connection.getAccountInfo(userTokenAccount);
  if (!userInfo) {
    throw new Error(
      `User USDC ATA ${userTokenAccount.toBase58()} does not exist. ` +
        `Send some USDC to ${keypair.publicKey.toBase58()} first.`,
    );
  }

  const setupIxs: TransactionInstruction[] = [];
  const vaultInfo = await connection.getAccountInfo(vaultTokenAccount);
  if (!vaultInfo) {
    setupIxs.push(
      createAssociatedTokenAccountInstruction(
        feePayer.publicKey, // payer = the paymaster, so user doesn't need SOL
        vaultTokenAccount,
        vault,
        usdcMint,
        tokenProgram,
        ASSOCIATED_TOKEN_PROGRAM_ID,
      ),
    );
  }

  let nonce = randomNonce();
  let stakeRecord: PublicKey;
  for (let attempt = 0; attempt < 5; attempt++) {
    const buf = Buffer.alloc(8);
    buf.writeBigUInt64LE(BigInt(nonce.toString()));
    const [pda] = PublicKey.findProgramAddressSync(
      [Buffer.from("stake_record"), buf],
      programId,
    );
    const existing = await connection.getAccountInfo(pda);
    if (!existing) {
      stakeRecord = pda;
      break;
    }
    nonce = randomNonce();
  }
  if (!stakeRecord!) {
    throw new Error("Failed to find an unused stake_record PDA after 5 tries.");
  }

  console.log("Staking (gasless)...");
  console.log("  Program:        ", programId.toBase58());
  console.log("  Fee payer:      ", feePayer.publicKey.toBase58());
  console.log("  User (auth):    ", keypair.publicKey.toBase58());
  console.log("  USDC mint:      ", usdcMint.toBase58());
  console.log("  Bridge fee:     ", bs.bridgeFee.toString());
  console.log("  Gasless fee:    ", bs.gaslessFee.toString());
  console.log("  Total deducted: ", bs.bridgeFee.add(bs.gaslessFee).toString());
  console.log("  Amount:         ", amount.toString());
  console.log("  Receiver (32B): ", "0x" + Buffer.from(receiver).toString("hex"));
  console.log("  Nonce:          ", nonce.toString());

  const stakeIx = await program.methods
    .stakeGasless(nonce, amount, receiver)
    .accounts({
      bridgeState,
      stakeRecord: stakeRecord!,
      user: keypair.publicKey,
      vault,
      usdcMint,
      userTokenAccount,
      vaultTokenAccount,
      tokenProgram,
      systemProgram: SystemProgram.programId,
    } as any)
    .instruction();

  const tx = new Transaction();
  for (const ix of setupIxs) tx.add(ix);
  tx.add(stakeIx);
  tx.feePayer = feePayer.publicKey;
  tx.recentBlockhash = (
    await connection.getLatestBlockhash("confirmed")
  ).blockhash;

  // Sign with both feePayer (pays SOL) and user (USDC authority). When they
  // are the same keypair, dedupe so we don't pass the same signer twice.
  const signers: Signer[] =
    feePayer.publicKey.equals(keypair.publicKey)
      ? [keypair]
      : [feePayer, keypair];
  tx.sign(...signers);

  const sig = await connection.sendRawTransaction(tx.serialize(), {
    skipPreflight: false,
  });
  await connection.confirmTransaction(sig, "confirmed");

  console.log("");
  console.log("TX:           ", sig);
  console.log("StakeRecord:  ", stakeRecord!.toBase58());
  console.log("SUCCESS");
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  if (e.logs) {
    console.error("Program logs:");
    for (const l of e.logs) console.error("  " + l);
  }
  process.exit(1);
});
