// withdraw-token.ts — Admin withdraws tokens from the bridge vault.
// Calls the program's `withdraw_token` instruction (timelock-protected).
//
// CLI:
//   --rpc-url, --keypair, --program-id   (standard, see client.ts)
//   --mint    <pubkey>                    token mint to withdraw
//   --amount  <u64 raw>                   amount in raw units
//   --to      <pubkey>                    recipient wallet (ATA resolved automatically)
//
// Output: JSON with { signature, newVaultBalance }

import { PublicKey, SystemProgram } from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAssociatedTokenAddressSync,
  createAssociatedTokenAccountInstruction,
  getMint,
} from "@solana/spl-token";
import * as anchor from "@coral-xyz/anchor";
import {
  createClient,
  getBridgeStatePda,
  getVaultPda,
  getTimelockPda,
  parseArgs,
} from "../client";
import * as crypto from "crypto";

function computeOpHash(parts: Buffer[]): Buffer {
  const h = crypto.createHash("sha256");
  for (const p of parts) h.update(p);
  return h.digest();
}

async function main() {
  const baseConfig = parseArgs();
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const mintStr = extra["mint"];
  const amountStr = extra["amount"];
  const toStr = extra["to"];
  if (!mintStr) throw new Error("Missing required arg: --mint");
  if (!amountStr) throw new Error("Missing required arg: --amount");
  if (!toStr) throw new Error("Missing required arg: --to");

  const mint = new PublicKey(mintStr);
  const amount = new anchor.BN(amountStr);
  const to = new PublicKey(toStr);
  if (amount.lten(0)) throw new Error("Amount must be > 0");

  const { program, programId, connection, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const vault = getVaultPda(programId);

  // Detect Token vs Token-2022
  const mintAccountInfo = await connection.getAccountInfo(mint);
  if (!mintAccountInfo) throw new Error(`Mint ${mint.toBase58()} not found`);
  const tokenProgram = mintAccountInfo.owner.equals(TOKEN_2022_PROGRAM_ID)
    ? TOKEN_2022_PROGRAM_ID
    : TOKEN_PROGRAM_ID;

  const mintInfo = await getMint(connection, mint, "confirmed", tokenProgram);
  const decimals = mintInfo.decimals;

  const vaultTokenAccount = getAssociatedTokenAddressSync(
    mint, vault, true, tokenProgram, ASSOCIATED_TOKEN_PROGRAM_ID,
  );
  const toTokenAccount = getAssociatedTokenAddressSync(
    mint, to, true, tokenProgram, ASSOCIATED_TOKEN_PROGRAM_ID,
  );

  // Compute timelock op hash (must match on-chain compute_op_hashv)
  const opHash = computeOpHash([
    Buffer.from("withdrawToken"),
    mint.toBuffer(),
    Buffer.from(amount.toArray("le", 8)),
    to.toBuffer(),
  ]);
  const timelockOp = getTimelockPda(programId, opHash);

  // If the recipient's ATA doesn't exist, create it first
  const toInfo = await connection.getAccountInfo(toTokenAccount);
  const preIxs: anchor.web3.TransactionInstruction[] = [];
  if (!toInfo) {
    console.log(`Recipient ATA ${toTokenAccount.toBase58()} does not exist; creating...`);
    preIxs.push(
      createAssociatedTokenAccountInstruction(
        keypair.publicKey, toTokenAccount, to, mint, tokenProgram, ASSOCIATED_TOKEN_PROGRAM_ID,
      ),
    );
  }

  const ix = await (program.methods as any)
    .withdrawToken(amount, to)
    .accounts({
      bridgeState,
      timelockOp,
      admin: keypair.publicKey,
      vault,
      tokenMint: mint,
      vaultTokenAccount,
      toTokenAccount,
      tokenProgram,
    })
    .instruction();

  const tx = new anchor.web3.Transaction().add(...preIxs, ix);
  const sig = await (anchor.getProvider() as anchor.AnchorProvider).sendAndConfirm(
    tx, [], { commitment: "confirmed" },
  );

  let newBal = "?";
  try {
    const bal = await connection.getTokenAccountBalance(vaultTokenAccount);
    newBal = bal.value.amount;
  } catch { /* ATA may be empty */ }

  console.log(JSON.stringify({
    signature: sig,
    newVaultBalance: newBal,
    mint: mint.toBase58(),
    to: to.toBase58(),
    decimals,
  }));
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
