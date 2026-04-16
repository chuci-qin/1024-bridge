import {
  createClient,
  getBridgeStatePda,
  parseArgs,
} from "../client";

async function main() {
  const baseConfig = parseArgs();
  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);

  console.log("Activating timelock...");

  const tx = await program.methods
    .activateTimelock()
    .accounts({
      admin: keypair.publicKey,
      bridgeState,
    } as any)
    .rpc();

  console.log("TX:", tx);
  console.log("SUCCESS");
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
