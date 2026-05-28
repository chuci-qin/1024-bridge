import * as anchor from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import * as fs from "fs";
import * as path from "path";

// The Anchor workspace builds two distinct programs:
//   - bridge1024      = leaf (single-peer, EVM-symmetric) → Solana / leaf SVM chains
//   - bridge1024_hub  = hub  (multi-peer)                 → 1024 chain
//
// Each emits its own IDL under contracts/svm/target/idl/. Every shell wrapper
// passes `--program-kind hub|leaf` so we load the right one. Loading the
// wrong IDL silently mis-encodes calldata (same `configure` discriminator but
// different arg layouts → guaranteed deserialization failure on-chain).
export type ProgramKind = "hub" | "leaf";

const IDL_DIR = path.resolve(
  __dirname,
  "../../../contracts/svm/target/idl",
);

function idlPathFor(kind: ProgramKind): string {
  return kind === "hub"
    ? path.join(IDL_DIR, "bridge1024_hub.json")
    : path.join(IDL_DIR, "bridge1024.json");
}

export interface ClientConfig {
  rpcUrl: string;
  keypairPath: string;
  programId: string;
  programKind: ProgramKind;
}

export function loadKeypair(keypairPath: string): Keypair {
  const raw = JSON.parse(fs.readFileSync(keypairPath, "utf-8"));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}

export function createClient(config: ClientConfig) {
  const connection = new Connection(config.rpcUrl, "confirmed");
  const keypair = loadKeypair(config.keypairPath);
  const wallet = new anchor.Wallet(keypair);
  const provider = new anchor.AnchorProvider(connection, wallet, {
    commitment: "confirmed",
  });
  anchor.setProvider(provider);

  const idlPath = idlPathFor(config.programKind);
  if (!fs.existsSync(idlPath)) {
    throw new Error(
      `IDL not found for program kind '${config.programKind}': ${idlPath}. ` +
        `Run 'anchor build -p ${
          config.programKind === "hub" ? "bridge1024_hub" : "bridge1024"
        }' first.`,
    );
  }
  const idl = JSON.parse(fs.readFileSync(idlPath, "utf-8"));
  const programId = new PublicKey(config.programId);
  // Anchor 0.30+ reads idl.address as program id and ignores the externally
  // provided programId. When the IDL's recorded address (from declare_id!()
  // / `anchor keys sync`) doesn't match the deployed address, the user-passed
  // programId must win — otherwise we'd call a non-existent program.
  idl.address = config.programId;
  const program = new anchor.Program(idl, provider);

  return { program, provider, connection, wallet, keypair, programId };
}

export function getBridgeStatePda(programId: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("bridge_state")],
    programId
  );
  return pda;
}

export function getVaultPda(programId: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    programId
  );
  return pda;
}

// Hub-only PDA. Leaf stores its single peer inline on BridgeState.
export function getPeerConfigPda(
  programId: PublicKey,
  chainId: number
): PublicKey {
  const buf = Buffer.alloc(8);
  buf.writeBigUInt64LE(BigInt(chainId));
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("peer_config"), buf],
    programId
  );
  return pda;
}

export function getTimelockPda(
  programId: PublicKey,
  opHash: Buffer
): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("timelock"), opHash],
    programId
  );
  return pda;
}

export function parseArgs(): ClientConfig {
  const args = process.argv.slice(2);
  const config: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    config[args[i].replace("--", "")] = args[i + 1];
  }
  const kindRaw = (config["program-kind"] || "").toLowerCase();
  let programKind: ProgramKind;
  if (kindRaw === "hub" || kindRaw === "leaf") {
    programKind = kindRaw;
  } else {
    // Back-compat default: most legacy callers (info / role ops) work on
    // either program; default to leaf so we keep talking to Solana correctly,
    // and let hub-only TS scripts pass --program-kind hub explicitly.
    programKind = "leaf";
  }
  return {
    rpcUrl: config["rpc-url"] || "",
    keypairPath: config["keypair"] || "",
    programId: config["program-id"] || "",
    programKind,
  };
}
