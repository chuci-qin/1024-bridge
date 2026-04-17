// 接受 admin 转移：使用 keypair（必须等于 pending_admin）签名调用 accept_admin。
//
// 注意：keypair 通常和现有 admin keypair 不同，需要在 config/<env>/.env
// 切换 SVM_KEYPAIR_PATH 到新 admin 的 keypair 后再运行此脚本。

import {
  createClient,
  getBridgeStatePda,
  parseArgs,
} from "../client";

async function main() {
  const baseConfig = parseArgs();
  const { program, programId, keypair } = createClient(baseConfig);
  const bridgeState = getBridgeStatePda(programId);

  const bs: any = await (program.account as any).bridgeState.fetch(bridgeState);
  console.log("Current admin:   ", bs.admin.toBase58());
  console.log("Pending admin:   ", bs.pendingAdmin.toBase58());
  console.log("Signer (newAdmin):", keypair.publicKey.toBase58());

  if (!bs.pendingAdmin.equals(keypair.publicKey)) {
    throw new Error("signer 不等于 pending_admin，无法 accept_admin");
  }

  const tx = await program.methods
    .acceptAdmin()
    .accounts({
      bridgeState,
      newAdmin: keypair.publicKey,
    } as any)
    .rpc();

  console.log("TX:", tx);
  console.log("SUCCESS");
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
