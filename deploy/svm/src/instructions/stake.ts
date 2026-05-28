// stake.ts — User-side `stake` instruction for the SVM bridge.
//
// Locks USDC into the program vault and emits a Staked event that the relayer
// picks up to unlock funds on the target chain.
//
// Hub  (bridge1024_hub): stake(nonce, amount, receiver, target_chain_id)
//   Requires a PeerConfig PDA for the target chain (used for per-peer fee &
//   max stake validation on-chain).
// Leaf (bridge1024):     stake(nonce, amount, receiver)
//   Single-peer model — target chain & peer contract are implicit in
//   BridgeState; no PeerConfig account is passed.
//
// Common boilerplate:
//   - Decode the receiver from either base58 (32B SVM pubkey) or hex (any 32B).
//   - Derive the user's USDC ATA and bail out if it doesn't exist.
//   - Derive the vault's USDC ATA; create it on-the-fly if missing
//     (Anchor doesn't init it for us — first stake otherwise reverts).
//   - Pick a random non-zero u64 nonce and derive the StakeRecord PDA.
//
// CLI:
//   --rpc-url, --keypair, --program-id, --program-kind   (see client.ts)
//   --target-chain-id   <u64>              (hub only — leaf ignores it)
//   --amount            <u64 raw USDC>     amount to stake
//   --receiver          <hex64 | base58>   destination on target chain
//
// Output: prints the tx signature and stake record info as plain text;
// stake.sh just tails the exit code.

import {
  PublicKey,
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
import {
  createClient,
  getBridgeStatePda,
  getVaultPda,
  getPeerConfigPda,
  parseArgs,
} from "../client";
import { randomBytes } from "crypto";

// Random non-zero u64 nonce (Buffer.readBigUInt64LE on 8 random bytes).
function randomNonce(): anchor.BN {
  for (let i = 0; i < 4; i++) {
    const buf = randomBytes(8);
    const v = buf.readBigUInt64LE(0);
    if (v !== 0n) return new anchor.BN(v.toString());
  }
  return new anchor.BN(1);
}

// Decode the user-supplied receiver into a 32-byte array. Accepts:
//   - hex (with or without 0x prefix), exactly 64 hex chars
//   - SVM base58 pubkey (32-44 chars), decoded via PublicKey
function decodeReceiver(input: string): number[] {
  const raw = input.trim();
  const hex = raw.startsWith("0x") || raw.startsWith("0X") ? raw.slice(2) : raw;
  if (/^[0-9a-fA-F]+$/.test(hex) && hex.length === 64) {
    return Array.from(Buffer.from(hex, "hex"));
  }
  const pk = new PublicKey(raw);
  return Array.from(pk.toBytes());
}

async function main() {
  const baseConfig = parseArgs();
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const isHub = baseConfig.programKind === "hub";

  const targetChainIdStr = extra["target-chain-id"];
  const amountStr = extra["amount"];
  const receiverStr = extra["receiver"];
  if (!amountStr || !receiverStr) {
    throw new Error("Missing required args: --amount, --receiver");
  }
  if (isHub && !targetChainIdStr) {
    throw new Error("--target-chain-id is required for hub stake.");
  }

  const targetChainId = isHub
    ? new anchor.BN(targetChainIdStr)
    : new anchor.BN(0);
  const amount = new anchor.BN(amountStr);
  const receiver = decodeReceiver(receiverStr);

  const { program, programId, connection, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const vault = getVaultPda(programId);
  const peerConfig = isHub
    ? getPeerConfigPda(programId, targetChainId.toNumber())
    : null;

  // Pull mint + verify peer + sanity-check inputs against on-chain state
  const bs: any = await (program.account as any).bridgeState.fetch(bridgeState);
  if (bs.usdcMint.equals(PublicKey.default)) {
    throw new Error("USDC mint not configured on bridge — run 'Configure' first.");
  }
  if (bs.isPaused) {
    throw new Error("Bridge is paused — cannot stake.");
  }
  const usdcMint: PublicKey = bs.usdcMint;

  if (isHub) {
    const pc: any = await (program.account as any).peerConfig.fetchNullable(
      peerConfig,
    );
    if (!pc) {
      throw new Error(
        `Peer chain ${targetChainId.toString()} is not registered on this hub.`,
      );
    }
  } else {
    // Leaf: verify single-peer configuration is set.
    const leafPeerChain: anchor.BN = bs.peerChainId;
    if (leafPeerChain.isZero()) {
      throw new Error(
        "Leaf bridge has no peer configured — run 'Configure' first.",
      );
    }
  }

  // Detect mint owner program (Token vs Token-2022) so the CPI uses the right one
  const mintAccountInfo = await connection.getAccountInfo(usdcMint);
  if (!mintAccountInfo) {
    throw new Error(`USDC mint ${usdcMint.toBase58()} not found on-chain.`);
  }
  const tokenProgram = mintAccountInfo.owner.equals(TOKEN_2022_PROGRAM_ID)
    ? TOKEN_2022_PROGRAM_ID
    : TOKEN_PROGRAM_ID;

  // ATAs: user (must exist + be funded) and vault (auto-create if missing)
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
    console.log(
      `Vault USDC ATA ${vaultTokenAccount.toBase58()} does not exist; creating...`,
    );
    setupIxs.push(
      createAssociatedTokenAccountInstruction(
        keypair.publicKey, // payer
        vaultTokenAccount,
        vault, // owner (PDA)
        usdcMint,
        tokenProgram,
        ASSOCIATED_TOKEN_PROGRAM_ID,
      ),
    );
  }

  // Pick a fresh nonce until the StakeRecord PDA is unused (collision is
  // astronomically rare for u64, but we'd rather retry than send a doomed tx)
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

  console.log("Staking...");
  console.log("  Program kind:   ", baseConfig.programKind);
  console.log("  Program:        ", programId.toBase58());
  console.log("  User:           ", keypair.publicKey.toBase58());
  console.log("  USDC mint:      ", usdcMint.toBase58());
  console.log("  User ATA:       ", userTokenAccount.toBase58());
  console.log("  Vault ATA:      ", vaultTokenAccount.toBase58());
  if (isHub) {
    console.log("  Target chain ID:", targetChainId.toString());
  } else {
    console.log("  Target chain ID:", "(implicit: BridgeState.peer_chain_id)");
  }
  console.log("  Amount:         ", amount.toString());
  console.log("  Receiver (32B): ", "0x" + Buffer.from(receiver).toString("hex"));
  console.log("  Nonce:          ", nonce.toString());

  // Build the stake instruction (manually so we can prepend the optional
  // create-ATA ix in a single atomic tx)
  let stakeIx: TransactionInstruction;
  if (isHub) {
    stakeIx = await program.methods
      .stake(nonce, amount, receiver, targetChainId)
      .accounts({
        bridgeState,
        peerConfig: peerConfig!,
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
  } else {
    stakeIx = await program.methods
      .stake(nonce, amount, receiver)
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
  }

  const tx = new Transaction();
  for (const ix of setupIxs) tx.add(ix);
  tx.add(stakeIx);

  const sig = await (program.provider as anchor.AnchorProvider).sendAndConfirm(
    tx,
    [keypair],
    { commitment: "confirmed" },
  );

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
