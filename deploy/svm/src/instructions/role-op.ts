// Role rotation 通用入口：proposeAdmin / setGuardian / setOperator / setRecovery
//
// 模式：
//   --mode auto      根据链上 timelock_active 自动选择执行 / 调度（默认）
//   --mode execute   仅执行（要求 timelock 未激活，或已调度且 ETA 已过）
//   --mode schedule  仅调度（要求 timelock 已激活，且未存在同 op_hash 的调度）
//
// op_hash = sha256(op_name_bytes || new_role_bytes)
// data    = op_name_bytes || new_role_bytes  （与合约 compute_op_hashv 一致）

import { PublicKey, SystemProgram } from "@solana/web3.js";
import { createHash } from "crypto";
import {
  createClient,
  getBridgeStatePda,
  getTimelockPda,
  parseArgs,
} from "../client";

type RoleOp = "proposeAdmin" | "setGuardian" | "setOperator" | "setRecovery";

const TIMELOCK_DELAY = 24 * 3600;
const TIMELOCK_GRACE = 48 * 3600;

function fmtUtc(unix: number): string {
  return new Date(unix * 1000).toISOString().replace("T", " ").replace(/\.\d+Z$/, " UTC");
}

async function main() {
  const baseConfig = parseArgs();
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }

  const op = extra["op"] as RoleOp;
  const target = new PublicKey(extra["target"]);
  const mode = (extra["mode"] || "auto") as "auto" | "execute" | "schedule";

  if (!["proposeAdmin", "setGuardian", "setOperator", "setRecovery"].includes(op)) {
    throw new Error(`unsupported op: ${op}`);
  }

  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);

  const data = Buffer.concat([Buffer.from(op), target.toBuffer()]);
  const opHash = createHash("sha256").update(data).digest();
  const timelockOpPda = getTimelockPda(programId, opHash);

  const bs: any = await (program.account as any).bridgeState.fetch(bridgeState);
  const timelockActive: boolean = bs.timelockActive;

  console.log("Role op:        ", op);
  console.log("Target:         ", target.toBase58());
  console.log("op_hash:        ", "0x" + opHash.toString("hex"));
  console.log("timelockActive: ", timelockActive);

  // 路径选择
  if (!timelockActive) {
    if (mode === "schedule") {
      throw new Error("timelock 未激活，无需 schedule，直接执行即可");
    }
    await runExecute(program, bridgeState, timelockOpPda, op, target, keypair.publicKey);
    return;
  }

  // timelock active：判断是否已调度
  const tlAccount = await (program.account as any).timelockOperation.fetchNullable(timelockOpPda);

  if (mode === "schedule" || (mode === "auto" && tlAccount === null)) {
    if (tlAccount !== null) {
      throw new Error(`已存在同 op_hash 的调度（eta=${fmtUtc(Number(tlAccount.eta))}）`);
    }
    await runSchedule(program, bridgeState, timelockOpPda, opHash, data, keypair.publicKey);
    return;
  }

  if (tlAccount === null) {
    throw new Error("操作未调度，先用 --mode schedule 调度");
  }

  // 检查 ETA
  const now = Math.floor(Date.now() / 1000);
  const eta = Number(tlAccount.eta);
  console.log("scheduled ETA:  ", fmtUtc(eta));
  console.log("expires after:  ", fmtUtc(eta + TIMELOCK_GRACE));

  if (now < eta) {
    const remain = eta - now;
    throw new Error(`未到执行时间，还差 ${Math.floor(remain / 3600)}h ${Math.floor((remain % 3600) / 60)}m`);
  }
  if (now > eta + TIMELOCK_GRACE) {
    throw new Error("操作已过期（超过 grace period 48h），需重新调度");
  }

  await runExecute(program, bridgeState, timelockOpPda, op, target, keypair.publicKey);
}

async function runSchedule(
  program: any,
  bridgeState: PublicKey,
  timelockOp: PublicKey,
  opHash: Buffer,
  data: Buffer,
  admin: PublicKey,
) {
  console.log("→ scheduling operation...");
  const tx = await program.methods
    .scheduleOperation(Array.from(opHash), Array.from(data))
    .accounts({
      bridgeState,
      timelockOp,
      admin,
      systemProgram: SystemProgram.programId,
    } as any)
    .rpc();
  console.log("TX:", tx);
  console.log("scheduled, executable after:", fmtUtc(Math.floor(Date.now() / 1000) + TIMELOCK_DELAY));
  console.log("SUCCESS");
}

async function runExecute(
  program: any,
  bridgeState: PublicKey,
  timelockOp: PublicKey,
  op: RoleOp,
  target: PublicKey,
  admin: PublicKey,
) {
  console.log("→ executing", op, "...");
  const accounts: any = { bridgeState, timelockOp, admin };
  let builder: any;
  switch (op) {
    case "proposeAdmin":
      builder = program.methods.proposeAdmin(target);
      break;
    case "setGuardian":
      builder = program.methods.setGuardian(target);
      break;
    case "setOperator":
      builder = program.methods.setOperator(target);
      break;
    case "setRecovery":
      builder = program.methods.setRecovery(target);
      break;
  }
  const tx = await builder.accounts(accounts).rpc();
  console.log("TX:", tx);
  console.log("SUCCESS");
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
