// fund-vault.ts — Top up the bridge program's USDC vault from the signer's
// own ATA. Used by deploy/svm/fund-vault.sh to give the hub (1024) — or any
// satellite SVM bridge — the USDC liquidity needed to honor cross-chain
// unlocks. The vault token account is the ATA owned by the off-curve
// `vault` PDA (seeds=[b"vault"]).
//
// What this script does:
//   1. Fetch BridgeState to learn the configured USDC mint.
//   2. Detect whether the mint is classic Token or Token-2022 (different
//      program IDs, different ATAs).
//   3. Derive the signer's ATA + the vault's ATA. Bail out if the signer
//      ATA is missing or under-funded — those are user errors that should
//      surface clearly, not be papered over.
//   4. If the vault ATA doesn't exist yet (very first top-up), prepend a
//      `createAssociatedTokenAccount` ix so the transfer succeeds atomically.
//   5. Send a plain SPL transferChecked for the requested raw amount.
//
// CLI:
//   --rpc-url, --keypair, --program-id     (standard, see client.ts)
//   --amount    <u64 raw USDC>             amount in raw units (6 decimals)
//
// Output: prints the tx signature on stdout for the wrapper to capture.

import {
  PublicKey,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAssociatedTokenAddressSync,
  createAssociatedTokenAccountInstruction,
  createTransferCheckedInstruction,
  getMint,
} from "@solana/spl-token";
import * as anchor from "@coral-xyz/anchor";
import {
  createClient,
  getBridgeStatePda,
  getVaultPda,
  parseArgs,
} from "../client";

async function main() {
  const baseConfig = parseArgs();
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const amountStr = extra["amount"];
  if (!amountStr) {
    throw new Error("Missing required arg: --amount (raw u64 USDC)");
  }
  const amount = new anchor.BN(amountStr);
  if (amount.lten(0)) {
    throw new Error("Amount must be > 0");
  }

  const { program, programId, connection, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const vault = getVaultPda(programId);

  const bs: any = await (program.account as any).bridgeState.fetch(bridgeState);
  if (bs.usdcMint.equals(PublicKey.default)) {
    throw new Error(
      "USDC mint not configured on bridge — run 'Configure' first.",
    );
  }
  const usdcMint: PublicKey = bs.usdcMint;

  // Detect Token vs Token-2022 by looking at who owns the mint account
  const mintAccountInfo = await connection.getAccountInfo(usdcMint);
  if (!mintAccountInfo) {
    throw new Error(`USDC mint ${usdcMint.toBase58()} not found on-chain.`);
  }
  const tokenProgram = mintAccountInfo.owner.equals(TOKEN_2022_PROGRAM_ID)
    ? TOKEN_2022_PROGRAM_ID
    : TOKEN_PROGRAM_ID;

  // Pull decimals — required by transferChecked (and surfaces wrong-mint bugs)
  const mintInfo = await getMint(connection, usdcMint, "confirmed", tokenProgram);
  const decimals = mintInfo.decimals;

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
    true, // vault is a PDA — owner is off-curve
    tokenProgram,
    ASSOCIATED_TOKEN_PROGRAM_ID,
  );

  // Sanity-check the source ATA before we waste a confirmation
  const userInfo = await connection.getAccountInfo(userTokenAccount);
  if (!userInfo) {
    throw new Error(
      `Signer USDC ATA ${userTokenAccount.toBase58()} does not exist. ` +
        `Send some USDC to ${keypair.publicKey.toBase58()} first.`,
    );
  }
  const userBal = await connection.getTokenAccountBalance(userTokenAccount);
  if (new anchor.BN(userBal.value.amount).lt(amount)) {
    throw new Error(
      `Insufficient USDC: have ${userBal.value.amount}, need ${amount.toString()} (raw).`,
    );
  }

  const setupIxs: TransactionInstruction[] = [];
  const vaultInfo = await connection.getAccountInfo(vaultTokenAccount);
  if (!vaultInfo) {
    console.log(
      `Vault USDC ATA ${vaultTokenAccount.toBase58()} does not exist; creating...`,
    );
    setupIxs.push(
      createAssociatedTokenAccountInstruction(
        keypair.publicKey, // payer
        vaultTokenAccount, // ata
        vault,             // owner (the PDA)
        usdcMint,
        tokenProgram,
        ASSOCIATED_TOKEN_PROGRAM_ID,
      ),
    );
  }

  const transferIx = createTransferCheckedInstruction(
    userTokenAccount,
    usdcMint,
    vaultTokenAccount,
    keypair.publicKey,
    BigInt(amount.toString()),
    decimals,
    [],
    tokenProgram,
  );

  const tx = new Transaction().add(...setupIxs, transferIx);
  const sig = await (anchor.getProvider() as anchor.AnchorProvider).sendAndConfirm(
    tx,
    [],
    { commitment: "confirmed" },
  );

  // Read post-balance for the wrapper to surface
  let newBal = "?";
  try {
    const bal = await connection.getTokenAccountBalance(vaultTokenAccount);
    newBal = bal.value.amount;
  } catch {
    // ATA somehow disappeared; not fatal for the print
  }

  console.log(JSON.stringify({
    signature: sig,
    vaultAta: vaultTokenAccount.toBase58(),
    vaultBalance: newBal,
    usdcMint: usdcMint.toBase58(),
    decimals,
  }));
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
