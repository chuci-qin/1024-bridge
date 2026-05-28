// configure-peer-rate-limits.ts — Hub-only: per-peer rate-limit configuration.
//
// configurePeerRateLimits(chainId, maxPerWindow, windowDuration,
//                         maxSingle, maxStake)
// — resets sliding window for the targeted PeerConfig PDA.

import * as anchor from "@coral-xyz/anchor";
import {
  createClient,
  getBridgeStatePda,
  getPeerConfigPda,
  getTimelockPda,
  parseArgs,
} from "../client";

async function main() {
  const baseConfig = parseArgs();
  if (baseConfig.programKind !== "hub") {
    throw new Error(
      "configurePeerRateLimits is a hub-only instruction (per-peer rate limits live on PeerConfig).",
    );
  }
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const chainId = new anchor.BN(extra["chain-id"]);
  const maxPerWindow = new anchor.BN(extra["max-per-window"]);
  const windowDuration = new anchor.BN(extra["window-duration"]);
  const maxSingle = new anchor.BN(extra["max-single"]);
  const maxStake = new anchor.BN(extra["max-stake"]);

  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const peerConfig = getPeerConfigPda(programId, chainId.toNumber());
  const dummyTimelock = getTimelockPda(programId, Buffer.alloc(32));

  console.log("Configuring peer rate limits...");
  console.log("  Chain ID:        ", chainId.toString());
  console.log("  Max per window:  ", maxPerWindow.toString());
  console.log("  Window duration: ", windowDuration.toString());
  console.log("  Max single:      ", maxSingle.toString());
  console.log("  Max stake:       ", maxStake.toString());

  const tx = await program.methods
    .configurePeerRateLimits(
      chainId,
      maxPerWindow,
      windowDuration,
      maxSingle,
      maxStake,
    )
    .accounts({
      admin: keypair.publicKey,
      bridgeState,
      peerConfig,
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
