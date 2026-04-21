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
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const chainId = new anchor.BN(extra["chain-id"]);
  const fee = new anchor.BN(extra["fee"]);

  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const peerConfig = getPeerConfigPda(programId, chainId.toNumber());
  const dummyTimelock = getTimelockPda(programId, Buffer.alloc(32));

  console.log("Configuring peer fee...");
  console.log("  Chain ID:", chainId.toString());
  console.log("  Fee:", fee.toString());

  const tx = await program.methods
    .configurePeerFee(chainId, fee)
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
