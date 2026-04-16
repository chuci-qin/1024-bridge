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
  console.log("  Max per window:", maxPerWindow.toString());
  console.log("  Window duration:", windowDuration.toString());
  console.log("  Max single:", maxSingle.toString());
  console.log("  Min reserve:", minReserve.toString());

  const tx = await program.methods
    .configureRateLimits(maxPerWindow, windowDuration, maxSingle, minReserve)
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
