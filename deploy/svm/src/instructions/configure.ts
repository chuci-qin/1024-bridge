// configure.ts — initial bridge configuration.
//
// Hub  (bridge1024_hub): configure(usdc_mint, local_chain_id)
//   Per-peer fields (peer_contract, peer_chain_id, bridge_fee, max_stake_amount)
//   live on PeerConfig PDAs, set via register_peer / configure_peer*.
//
// Leaf (bridge1024):     configure(usdc_mint, peer_contract[32], local_chain_id,
//                                  peer_chain_id, bridge_fee)
//   Single-peer model, mirrors the EVM Bridge1024.configure(...) layout.

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

  console.log("Configuring bridge1024", `(${baseConfig.programKind})...`);
  console.log("  Program kind:    ", baseConfig.programKind);
  console.log("  USDC mint:       ", usdcMint.toBase58());
  console.log("  Local chain ID:  ", localChainId.toString());

  let tx: string;
  if (baseConfig.programKind === "hub") {
    tx = await program.methods
      .configure(usdcMint, localChainId)
      .accounts({
        admin: keypair.publicKey,
        bridgeState,
        timelockOp: dummyTimelock,
      } as any)
      .rpc();
  } else {
    const peerContractHex = (extra["peer-contract"] || "").replace(/^0x/, "");
    const peerChainIdStr = extra["peer-chain-id"];
    const bridgeFeeStr = extra["bridge-fee"];
    if (peerContractHex.length !== 64) {
      throw new Error(
        "--peer-contract is required for leaf configure: 64 hex chars (no 0x).",
      );
    }
    if (!peerChainIdStr) {
      throw new Error("--peer-chain-id is required for leaf configure.");
    }
    if (bridgeFeeStr === undefined) {
      throw new Error(
        "--bridge-fee is required for leaf configure (raw u64, 0 = no fee).",
      );
    }
    const peerContract = Array.from(Buffer.from(peerContractHex, "hex"));
    const peerChainId = new anchor.BN(peerChainIdStr);
    const bridgeFee = new anchor.BN(bridgeFeeStr);

    console.log("  Peer contract:   ", "0x" + peerContractHex);
    console.log("  Peer chain ID:   ", peerChainId.toString());
    console.log("  Bridge fee:      ", bridgeFee.toString());

    tx = await program.methods
      .configure(usdcMint, peerContract, localChainId, peerChainId, bridgeFee)
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
