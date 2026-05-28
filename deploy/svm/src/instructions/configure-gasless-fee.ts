// configure-gasless-fee.ts — Leaf-only: update the gasless service fee.
//
// Mirrors EVM Bridge1024.configureGaslessFee(uint64). Setting fee = 0 disables
// the gasless path (stake_gasless reverts GaslessDisabled) without touching
// the plain stake path.

import * as anchor from "@coral-xyz/anchor";
import {
  createClient,
  getBridgeStatePda,
  getTimelockPda,
  parseArgs,
} from "../client";

async function main() {
  const baseConfig = parseArgs();
  if (baseConfig.programKind !== "leaf") {
    throw new Error(
      "configureGaslessFee is a leaf-only instruction (hub has no gasless path).",
    );
  }
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const fee = new anchor.BN(extra["fee"]);

  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const dummyTimelock = getTimelockPda(programId, Buffer.alloc(32));

  console.log("Configuring gasless fee (leaf)...");
  console.log("  Fee:", fee.toString());
  if (fee.isZero()) {
    console.log("  WARNING: setting gasless_fee = 0 disables the gasless path.");
  }

  const tx = await program.methods
    .configureGaslessFee(fee)
    .accounts({
      admin: keypair.publicKey,
      bridgeState,
      timelockOp: dummyTimelock,
    } as any)
    .rpc();

  console.log("TX:", tx);
  console.log("SUCCESS");
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
