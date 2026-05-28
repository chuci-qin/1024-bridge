import { PublicKey, SystemProgram } from "@solana/web3.js";
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
      "registerPeer is a hub-only instruction (multi-peer); leaf (bridge1024) " +
        "uses single-peer configure(...). Pass --program-kind hub or target 1024_*.",
    );
  }
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const chainId = new anchor.BN(extra["chain-id"]);
  const peerContractHex = extra["peer-contract"];
  const bridgeFee = new anchor.BN(extra["bridge-fee"]);
  const maxStakeAmount = new anchor.BN(extra["max-stake-amount"]);

  // peer-contract is 32 bytes hex (no 0x prefix)
  const peerContract = Array.from(Buffer.from(peerContractHex, "hex"));

  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  const peerConfig = getPeerConfigPda(programId, chainId.toNumber());
  const dummyTimelock = getTimelockPda(programId, Buffer.alloc(32));

  console.log("Registering peer...");
  console.log("  Chain ID:", chainId.toString());
  console.log("  Peer contract:", peerContractHex);
  console.log("  Bridge fee:", bridgeFee.toString());
  console.log("  Max stake amount:", maxStakeAmount.toString());

  const tx = await program.methods
    .registerPeer(chainId, peerContract, bridgeFee, maxStakeAmount)
    .accounts({
      admin: keypair.publicKey,
      bridgeState,
      peerConfig,
      timelockOp: dummyTimelock,
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
