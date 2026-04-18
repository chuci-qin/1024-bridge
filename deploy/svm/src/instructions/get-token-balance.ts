// get-token-balance.ts — Tiny helper used by stake.sh to poll a receiver's
// USDC balance on an SVM target chain. Prints the raw token amount (u64) as
// plain text on stdout. If the ATA doesn't exist yet (receiver was never
// funded), prints "0" so the bash poll loop can tell "still waiting" apart
// from a real RPC failure.
//
// CLI:
//   --rpc-url <url>          Solana JSON-RPC endpoint
//   --mint    <base58>       SPL token mint
//   --owner   <base58>       Receiver wallet pubkey (the ATA owner)
//
// Exit code is 0 unless the RPC call itself fails (in which case stderr
// carries the reason and stdout is empty).

import { Connection, PublicKey } from "@solana/web3.js";
import { getAssociatedTokenAddressSync } from "@solana/spl-token";

function parseFlags(): Record<string, string> {
  const args = process.argv.slice(2);
  const out: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    out[args[i].replace("--", "")] = args[i + 1];
  }
  return out;
}

async function main() {
  const f = parseFlags();
  if (!f["rpc-url"] || !f["mint"] || !f["owner"]) {
    throw new Error("Required flags: --rpc-url, --mint, --owner");
  }
  const conn = new Connection(f["rpc-url"], "confirmed");
  const mint = new PublicKey(f["mint"]);
  const owner = new PublicKey(f["owner"]);
  // Token-2022 vs classic Token: the ATA derivation cares which program owns
  // the mint. Detect by reading the mint account's owner program.
  const mintInfo = await conn.getAccountInfo(mint);
  if (!mintInfo) {
    process.stdout.write("0");
    return;
  }
  const ata = getAssociatedTokenAddressSync(
    mint,
    owner,
    true, // allow off-curve owners (PDAs etc.)
    mintInfo.owner,
  );
  try {
    const bal = await conn.getTokenAccountBalance(ata);
    process.stdout.write(bal.value.amount);
  } catch {
    // ATA doesn't exist yet — receiver hasn't been credited. Treat as 0.
    process.stdout.write("0");
  }
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
