// 读取 BridgeState + Vault USDC 余额 + 指定 peer 链路的 PeerConfig，
// 以 JSON 格式输出到 stdout。bash 脚本通过 jq 解析后用于：
//   - manage-roles.sh 的角色重叠预检（仅消费 admin/guardian/operator/recovery/pending 字段）
//   - info.sh 的链上状态展示（消费全部字段）

import { PublicKey } from "@solana/web3.js";
import { getAssociatedTokenAddressSync } from "@solana/spl-token";
import {
  createClient,
  getBridgeStatePda,
  getVaultPda,
  getPeerConfigPda,
  parseArgs,
} from "../client";

async function main() {
  const baseConfig = parseArgs();

  // 解析额外的 --peer-chain-ids "421614,11155111,84532"
  const args = process.argv.slice(2);
  const extra: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    extra[args[i].replace("--", "")] = args[i + 1];
  }
  const peerChainIds: number[] = (extra["peer-chain-ids"] || "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean)
    .map(Number)
    .filter((n) => Number.isFinite(n) && n > 0);

  const { program, programId, connection } = createClient(baseConfig);
  const bsPda = getBridgeStatePda(programId);
  const vaultPda = getVaultPda(programId);

  const bs: any = await (program.account as any).bridgeState.fetch(bsPda);

  // Vault USDC ATA 余额（vault PDA 是 owner）
  let vaultAta = "";
  let vaultBalance = "0";
  if (!bs.usdcMint.equals(PublicKey.default)) {
    const ata = getAssociatedTokenAddressSync(bs.usdcMint, vaultPda, true);
    vaultAta = ata.toBase58();
    try {
      const bal = await connection.getTokenAccountBalance(ata);
      vaultBalance = bal.value.amount;
    } catch {
      // ATA 还没创建（首次部署/configure 后未发生 stake）
      vaultBalance = "0";
    }
  }

  // PeerConfig 列表：逐个 chainId 查 PDA，存在的才返回
  const peers: any[] = [];
  for (const cid of peerChainIds) {
    const pda = getPeerConfigPda(programId, cid);
    try {
      const pc: any = await (program.account as any).peerConfig.fetchNullable(
        pda,
      );
      if (pc) {
        // peer_contract 是 [u8;32]：EVM 链右对齐 20B（前 12B 为 0），SVM 链是原生 32B 公钥
        const raw = Buffer.from(pc.peerContract);
        const hex = "0x" + raw.toString("hex");
        const isEvmRightAligned = raw.slice(0, 12).every((b) => b === 0);
        const peerContractEvm = isEvmRightAligned
          ? "0x" + raw.slice(12).toString("hex")
          : null;
        const peerContractSvm = new PublicKey(raw).toBase58();
        peers.push({
          chainId: pc.chainId.toString(),
          pda: pda.toBase58(),
          peerContract: hex,
          peerContractEvm,
          peerContractSvm,
          bridgeFee: pc.bridgeFee.toString(),
          maxStakeAmount: pc.maxStakeAmount.toString(),
          maxUnlockPerWindow: pc.maxUnlockPerWindow.toString(),
          windowDuration: pc.windowDuration.toString(),
          maxSingleUnlock: pc.maxSingleUnlock.toString(),
          currentWindowStart: pc.currentWindowStart.toString(),
          currentWindowUsage: pc.currentWindowUsage.toString(),
          previousWindowUsage: pc.previousWindowUsage.toString(),
        });
      }
    } catch {
      // 忽略：PDA 不存在或反序列化失败
    }
  }

  console.log(
    JSON.stringify({
      programId: programId.toBase58(),
      bridgeStatePda: bsPda.toBase58(),
      vaultPda: vaultPda.toBase58(),
      vaultAta,
      vaultBalance,
      // ── 角色 ──
      admin: bs.admin.toBase58(),
      guardian: bs.guardian.toBase58(),
      operator: bs.operator.toBase58(),
      recovery: bs.recovery.toBase58(),
      pending: bs.pendingAdmin.toBase58(),
      // ── 配置 ──
      usdcMint: bs.usdcMint.toBase58(),
      localChainId: bs.localChainId.toString(),
      vaultBump: bs.vaultBump,
      // ── 标志位 ──
      timelockActive: bs.timelockActive,
      isPaused: bs.isPaused,
      // ── 全局速率限制 ──
      maxUnlockPerWindow: bs.maxUnlockPerWindow.toString(),
      windowDuration: bs.windowDuration.toString(),
      maxSingleUnlock: bs.maxSingleUnlock.toString(),
      minimumReserve: bs.minimumReserve.toString(),
      currentWindowStart: bs.currentWindowStart.toString(),
      currentWindowUsage: bs.currentWindowUsage.toString(),
      previousWindowUsage: bs.previousWindowUsage.toString(),
      // ── 中继器 ──
      relayers: (bs.relayers as PublicKey[]).map((r) => r.toBase58()),
      // ── Peer 链路（仅返回查到的）──
      peers,
    }),
  );
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
