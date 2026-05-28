// configure-rate-limits.ts — global rate-limit configuration.
//
// Hub  (bridge1024_hub): configureRateLimits(max_per_window, window_duration,
//                                            max_single, min_reserve)
//   (max_stake is per-peer on the hub, see configure_peer_rate_limits.)
// Leaf (bridge1024):     configureRateLimits(max_per_window, window_duration,
//                                            max_single, max_stake, min_reserve)

import * as anchor from "@coral-xyz/anchor";
import {
  createClient,
  getBridgeStatePda,
  getTimelockPda,
  parseArgs,
} from "../client";

async function main() {
  const baseConfig = parseArgs();
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const maxPerWindow = new anchor.BN(extra["max-per-window"]);
  const windowDuration = new anchor.BN(extra["window-duration"]);
  const maxSingle = new anchor.BN(extra["max-single"]);
  const minReserve = new anchor.BN(extra["min-reserve"]);

  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const dummyTimelock = getTimelockPda(programId, Buffer.alloc(32));

  console.log("Configuring rate limits...");
  console.log("  Program kind:    ", baseConfig.programKind);
  console.log("  Max per window:  ", maxPerWindow.toString());
  console.log("  Window duration: ", windowDuration.toString());
  console.log("  Max single:      ", maxSingle.toString());
  console.log("  Min reserve:     ", minReserve.toString());

  let tx: string;
  if (baseConfig.programKind === "hub") {
    tx = await program.methods
      .configureRateLimits(maxPerWindow, windowDuration, maxSingle, minReserve)
      .accounts({
        admin: keypair.publicKey,
        bridgeState,
        timelockOp: dummyTimelock,
      } as any)
      .rpc();
  } else {
    const maxStakeStr = extra["max-stake"];
    if (maxStakeStr === undefined) {
      throw new Error(
        "--max-stake is required for leaf configure-rate-limits (raw u64, 0 = unlimited).",
      );
    }
    const maxStake = new anchor.BN(maxStakeStr);
    console.log("  Max stake:       ", maxStake.toString());

    tx = await program.methods
      .configureRateLimits(
        maxPerWindow,
        windowDuration,
        maxSingle,
        maxStake,
        minReserve,
      )
      .accounts({
        admin: keypair.publicKey,
        bridgeState,
        timelockOp: dummyTimelock,
      } as any)
      .rpc();
  }

  console.log("TX:", tx);
  console.log("SUCCESS");
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
