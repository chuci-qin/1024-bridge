// rotate_relayer 指令脚本：原子替换一个 relayer。
// 用法（真发）：
//   npx ts-node src/instructions/rotate-relayer.ts \
//     --rpc-url <RPC> --keypair <admin.json> --program-id <PID> \
//     --old <OLD_PUBKEY> --new <NEW_PUBKEY>
// 用法（dry-run，仅模拟，不上链）：
//   ... 末尾追加 --dry-run true

import { PublicKey } from "@solana/web3.js";
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

  const oldRelayer = new PublicKey(extra["old"]);
  const newRelayer = new PublicKey(extra["new"]);
  const dryRun = (extra["dry-run"] || "").toLowerCase() === "true";

  const { program, programId, keypair, connection } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);
  // timelock 未激活时合约不会真去解析 timelockOp 账户，但 IDL 仍要求传一个
  // 已存在或属于程序的 PDA。用 empty-hash 推出的 PDA（同 add-relayer.ts）即可。
  const dummyTimelock = getTimelockPda(programId, Buffer.alloc(32));

  console.log("Rotating relayer");
  console.log("  Program     :", programId.toBase58());
  console.log("  Admin       :", keypair.publicKey.toBase58());
  console.log("  BridgeState :", bridgeState.toBase58());
  console.log("  Old relayer :", oldRelayer.toBase58());
  console.log("  New relayer :", newRelayer.toBase58());
  console.log("  Mode        :", dryRun ? "DRY-RUN (simulate only)" : "SEND");

  const method = program.methods
    .rotateRelayer(oldRelayer, newRelayer)
    .accounts({
      admin: keypair.publicKey,
      bridgeState,
      timelockOp: dummyTimelock,
    } as any);

  if (dryRun) {
    const tx = await method.transaction();
    tx.feePayer = keypair.publicKey;
    tx.recentBlockhash = (
      await connection.getLatestBlockhash("finalized")
    ).blockhash;
    tx.sign(keypair);
    const sim = await connection.simulateTransaction(tx);
    console.log(
      "\nsimulate.err   :",
      sim.value.err === null ? "null (OK)" : JSON.stringify(sim.value.err),
    );
    console.log("simulate.unitsConsumed:", sim.value.unitsConsumed ?? "n/a");
    console.log("simulate.logs:");
    for (const line of sim.value.logs ?? []) console.log("  " + line);
    if (sim.value.err !== null) {
      process.exitCode = 2;
    }
    return;
  }

  const tx = await method.rpc();
  console.log("TX:", tx);
  console.log("SUCCESS");
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
