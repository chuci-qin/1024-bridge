import { PublicKey, SystemProgram } from "@solana/web3.js";
import {
  createClient,
  getBridgeStatePda,
  getVaultPda,
  parseArgs,
} from "../client";

async function main() {
  const baseConfig = parseArgs();
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const guardian = new PublicKey(extra["guardian"]);
  const operator = new PublicKey(extra["operator"]);
  const recovery = new PublicKey(extra["recovery"]);

  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const vault = getVaultPda(programId);

  console.log("Initializing bridge1024...");
  console.log("  Bridge state PDA:", bridgeState.toBase58());
  console.log("  Vault PDA:", vault.toBase58());
  console.log("  Admin:", keypair.publicKey.toBase58());
  console.log("  Guardian:", guardian.toBase58());
  console.log("  Operator:", operator.toBase58());
  console.log("  Recovery:", recovery.toBase58());

  const tx = await program.methods
    .initialize(guardian, operator, recovery)
    .accounts({
      admin: keypair.publicKey,
      bridgeState,
      vault,
      systemProgram: SystemProgram.programId,
    } as any)
    .rpc();

  console.log("TX:", tx);
  console.log("SUCCESS");
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
