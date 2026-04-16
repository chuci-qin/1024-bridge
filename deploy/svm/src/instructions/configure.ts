import { PublicKey } from "@solana/web3.js";
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

  const usdcMint = new PublicKey(extra["usdc-mint"]);
  const localChainId = new anchor.BN(extra["local-chain-id"]);

  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const dummyTimelock = getTimelockPda(programId, Buffer.alloc(32));

  console.log("Configuring bridge1024...");
  console.log("  USDC mint:", usdcMint.toBase58());
  console.log("  Local chain ID:", localChainId.toString());

  const tx = await program.methods
    .configure(usdcMint, localChainId)
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
