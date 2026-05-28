// configure-bridge-fee.ts — Leaf-only: update bridge fee on BridgeState.
//
// Hub keeps fees per-peer on PeerConfig (use configure-peer-fee.ts instead).
// Leaf stores a single `bridge_fee` inline on BridgeState; this instruction
// is the EVM-symmetric `configureBridgeFee(fee)` counterpart.

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
      "configureBridgeFee is a leaf-only instruction. On the hub, use " +
        "configurePeerFee(chain_id, fee) per peer.",
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

  console.log("Configuring bridge fee (leaf)...");
  console.log("  Fee:", fee.toString());

  const tx = await program.methods
    .configureBridgeFee(fee)
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
