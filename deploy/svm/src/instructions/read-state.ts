// read-state.ts — dumps BridgeState (+ vault USDC balance, peers) as JSON to
// stdout. Shell wrappers (info.sh / stake.sh / role ops) jq the output.
//
// Program-kind aware:
//   - hub:  scans `PeerConfig` PDAs for the chain IDs passed via
//           --peer-chain-ids and returns each as a `peers[]` entry.
//   - leaf: peer info lives inline on BridgeState (peer_chain_id, peer_contract,
//           bridge_fee, max_stake_amount, gasless_fee). We synthesize a
//           one-element `peers[]` matching the hub schema so downstream
//           consumers don't need to special-case kind, plus surface
//           `gaslessFee` at top level (leaf-only field).

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

  // Vault USDC ATA balance (vault PDA is owner)
  // We must use the ATA derived from the mint's owning program (legacy Token
  // or Token-2022); otherwise on a Token-2022 mint we'd compute a different,
  // non-existent address and getTokenAccountBalance would throw, hiding the
  // real balance under a perpetual 0.
  let vaultAta = "";
  let vaultBalance = "0";
  if (!bs.usdcMint.equals(PublicKey.default)) {
    const mintInfo = await connection.getAccountInfo(bs.usdcMint);
    if (mintInfo) {
      const ata = getAssociatedTokenAddressSync(
        bs.usdcMint,
        vaultPda,
        true,
        mintInfo.owner, // = TOKEN_PROGRAM_ID or TOKEN_2022_PROGRAM_ID
      );
      vaultAta = ata.toBase58();
      try {
        const bal = await connection.getTokenAccountBalance(ata);
        vaultBalance = bal.value.amount;
      } catch {
        // ATA not yet created (no stake / fund-vault since first deploy)
        vaultBalance = "0";
      }
    }
    // mintInfo == null: mint doesn't exist on chain; leave vaultAta empty + balance 0
  }

  // Encode each peer entry into the shape that downstream consumers expect.
  // Keep `chainId` as a string so jq comparisons in shell stay uniform.
  type PeerOut = {
    chainId: string;
    pda: string | null;
    peerContract: string;
    peerContractEvm: string | null;
    peerContractSvm: string;
    bridgeFee: string;
    maxStakeAmount: string;
    maxUnlockPerWindow: string;
    windowDuration: string;
    maxSingleUnlock: string;
    currentWindowStart: string;
    currentWindowUsage: string;
    previousWindowUsage: string;
  };

  function peerContractDisplay(raw: Buffer) {
    const hex = "0x" + raw.toString("hex");
    const isEvmRightAligned = raw.slice(0, 12).every((b) => b === 0);
    const peerContractEvm = isEvmRightAligned
      ? "0x" + raw.slice(12).toString("hex")
      : null;
    const peerContractSvm = new PublicKey(raw).toBase58();
    return { hex, peerContractEvm, peerContractSvm };
  }

  const peers: PeerOut[] = [];

  if (baseConfig.programKind === "hub") {
    // Hub: scan PeerConfig PDAs for the requested chain IDs.
    for (const cid of peerChainIds) {
      const pda = getPeerConfigPda(programId, cid);
      try {
        const pc: any = await (program.account as any).peerConfig.fetchNullable(
          pda,
        );
        if (pc) {
          const raw = Buffer.from(pc.peerContract);
          const d = peerContractDisplay(raw);
          peers.push({
            chainId: pc.chainId.toString(),
            pda: pda.toBase58(),
            peerContract: d.hex,
            peerContractEvm: d.peerContractEvm,
            peerContractSvm: d.peerContractSvm,
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
        // Ignore: PDA not found or deserialization failed
      }
    }
  } else {
    // Leaf: synthesize a one-element peers[] from BridgeState fields so
    // downstream callers (info.sh / stake.sh) don't need to know about the
    // split. peer_chain_id == 0 means "not yet configured" — emit nothing.
    if (!bs.peerChainId.isZero()) {
      const raw = Buffer.from(bs.peerContract);
      const d = peerContractDisplay(raw);
      peers.push({
        chainId: bs.peerChainId.toString(),
        pda: null, // leaf has no PeerConfig PDA
        peerContract: d.hex,
        peerContractEvm: d.peerContractEvm,
        peerContractSvm: d.peerContractSvm,
        bridgeFee: bs.bridgeFee.toString(),
        maxStakeAmount: bs.maxStakeAmount.toString(),
        // Leaf has a single rate-limit set on BridgeState; mirror it here so
        // stake.sh / info.sh can use the same .peers[] shape it uses on hub.
        maxUnlockPerWindow: bs.maxUnlockPerWindow.toString(),
        windowDuration: bs.windowDuration.toString(),
        maxSingleUnlock: bs.maxSingleUnlock.toString(),
        currentWindowStart: bs.currentWindowStart.toString(),
        currentWindowUsage: bs.currentWindowUsage.toString(),
        previousWindowUsage: bs.previousWindowUsage.toString(),
      });
    }
  }

  // Top-level snapshot. Most fields are common to both kinds. Leaf-only
  // surfaces gaslessFee; hub-only surfaces nothing extra (peer fields are
  // in peers[]).
  const out: Record<string, any> = {
    programKind: baseConfig.programKind,
    programId: programId.toBase58(),
    bridgeStatePda: bsPda.toBase58(),
    vaultPda: vaultPda.toBase58(),
    vaultAta,
    vaultBalance,
    // ── Roles ──
    admin: bs.admin.toBase58(),
    guardian: bs.guardian.toBase58(),
    operator: bs.operator.toBase58(),
    recovery: bs.recovery.toBase58(),
    pending: bs.pendingAdmin.toBase58(),
    // ── Config ──
    usdcMint: bs.usdcMint.toBase58(),
    localChainId: bs.localChainId.toString(),
    vaultBump: bs.vaultBump,
    // ── Flags ──
    timelockActive: bs.timelockActive,
    isPaused: bs.isPaused,
    // ── Global rate limits (both kinds have them on BridgeState) ──
    maxUnlockPerWindow: bs.maxUnlockPerWindow.toString(),
    windowDuration: bs.windowDuration.toString(),
    maxSingleUnlock: bs.maxSingleUnlock.toString(),
    minimumReserve: bs.minimumReserve.toString(),
    currentWindowStart: bs.currentWindowStart.toString(),
    currentWindowUsage: bs.currentWindowUsage.toString(),
    previousWindowUsage: bs.previousWindowUsage.toString(),
    // ── Relayers ──
    relayers: (bs.relayers as PublicKey[]).map((r) => r.toBase58()),
    // ── Peers (hub: from PDAs; leaf: synthesized 0/1 entries from BridgeState) ──
    peers,
  };

  if (baseConfig.programKind === "leaf") {
    // Leaf-specific fields. EVM-symmetric configure stores these directly on
    // BridgeState (no separate PeerConfig).
    out.bridgeFee = bs.bridgeFee.toString();
    out.gaslessFee = bs.gaslessFee.toString();
    out.maxStakeAmount = bs.maxStakeAmount.toString();
    out.peerChainId = bs.peerChainId.toString();
    out.peerContract =
      "0x" + Buffer.from(bs.peerContract).toString("hex");
  }

  console.log(JSON.stringify(out));
}

main().catch((e) => {
  console.error("FAILED:", e.message || e);
  process.exit(1);
});
