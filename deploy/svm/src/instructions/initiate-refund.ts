// initiate-refund.ts — docs1024#1125 refund path, STEP 2 of 3 (1024 hub, operator key).
//
// Step 1 (deploy/ops/evm-skip-nonces.sh) must have skipped the nonce on the TARGET
// chain. This script re-checks that on the target for EVERY nonce and refuses
// otherwise: a hub refund while the target nonce is still open is a double spend
// waiting for a relayer to wake up (the hub program cannot see the target chain).
// Step 3 (execute_refund after REFUND_DELAY, vault re-credit, DB `refunded`) is done
// automatically by 1024-core relayer1 StakedResolver as the stake owner.
//
// CLI (dry run unless --apply true):
//   npx ts-node src/instructions/initiate-refund.ts \
//     --rpc-url <1024 rpc> --keypair <operator.json> --program-id <hub> --program-kind hub \
//     --nonces-file nonces.txt --target-rpc <evm rpc> --target-bridge 0x... [--apply true]
// nonces.txt: `<target_chain_id>\t<nonce>` per line (hub_initiate_refund_nonces.txt or
// target_skip_nonces.txt from 1024-core scripts/bridge_1125_staked_disposition.py).

import * as fs from "fs";
import * as anchor from "@coral-xyz/anchor";
import { PublicKey } from "@solana/web3.js";
import { createClient, getBridgeStatePda, parseArgs } from "../client";

// keccak256("nonceConfirmations(uint64)")[0..4]
const SEL_NONCE_CONFIRMATIONS = "0x1164aafd";

async function targetState(rpc: string, bridge: string, nonce: bigint) {
  const data = SEL_NONCE_CONFIRMATIONS + nonce.toString(16).padStart(64, "0");
  const r = await fetch(rpc, {
    method: "POST",
    headers: { "content-type": "application/json", "user-agent": "1024-ops/initiate-refund" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "eth_call", params: [{ to: bridge, data }, "latest"] }),
  });
  const j: any = await r.json();
  if (j.error) throw new Error(`eth_call: ${JSON.stringify(j.error)}`);
  const hex: string = j.result.slice(2);
  return { processed: hex.slice(62, 64) === "01", unlocked: hex.slice(126, 128) === "01" };
}

function stakeRecordPda(programId: PublicKey, nonce: bigint): PublicKey {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(nonce);
  return PublicKey.findProgramAddressSync([Buffer.from("stake_record"), b], programId)[0];
}

async function main() {
  const base = parseArgs();
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) extra[args[i].replace("--", "")] = args[i + 1];
  const apply = extra["apply"] === "true";
  const targetRpc = extra["target-rpc"];
  const targetBridge = extra["target-bridge"];
  if (!extra["nonces-file"] || !targetRpc || !targetBridge) {
    throw new Error("need --nonces-file, --target-rpc, --target-bridge");
  }
  if (base.programKind !== "hub") throw new Error("initiate_refund is a hub instruction: --program-kind hub");

  const { program, programId, keypair } = createClient(base);
  const bridgeState = getBridgeStatePda(programId);
  const bs: any = await (program.account as any).bridgeState.fetch(bridgeState);
  if (apply && !bs.operator.equals(keypair.publicKey)) {
    throw new Error(`signer ${keypair.publicKey.toBase58()} is not the hub operator ${bs.operator.toBase58()}`);
  }

  const lines = fs.readFileSync(extra["nonces-file"], "utf-8").split("\n").filter((l) => l.trim());
  const tally: Record<string, number> = {};
  for (const line of lines) {
    const nonce = BigInt(line.split("\t").pop()!.trim());
    const t = await targetState(targetRpc, targetBridge, nonce);
    const pda = stakeRecordPda(programId, nonce);
    const sr: any = await (program.account as any).stakeRecord.fetchNullable(pda);
    let verdict: string;
    if (t.unlocked) verdict = "REFUSE_TARGET_UNLOCKED";
    else if (!t.processed) verdict = "REFUSE_TARGET_NOT_SKIPPED";
    else if (!sr) verdict = "REFUSE_NO_STAKE_RECORD";
    else if (sr.refunded) verdict = "ALREADY_REFUNDED";
    else if (!new anchor.BN(sr.refundInitiatedAt).isZero()) verdict = "ALREADY_INITIATED";
    else verdict = apply ? "INITIATED" : "WOULD_INITIATE";
    if (verdict === "INITIATED") {
      const sig = await (program.methods as any)
        .initiateRefund(new anchor.BN(nonce.toString()))
        .accounts({ bridgeState, stakeRecord: pda, operator: keypair.publicKey })
        .rpc();
      console.log(`${nonce}\t${verdict}\t${sig}`);
    } else {
      console.log(`${nonce}\t${verdict}`);
    }
    tally[verdict] = (tally[verdict] || 0) + 1;
  }
  console.log(JSON.stringify({ apply, tally }));
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
