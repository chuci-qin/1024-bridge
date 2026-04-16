import * as anchor from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import * as fs from "fs";
import * as path from "path";

const IDL_PATH = path.resolve(
  __dirname,
  "../../../contracts/svm/target/idl/bridge1024.json"
);

export interface ClientConfig {
  rpcUrl: string;
  keypairPath: string;
  programId: string;
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

  const idl = JSON.parse(fs.readFileSync(IDL_PATH, "utf-8"));
  const programId = new PublicKey(config.programId);
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
  return {
    rpcUrl: config["rpc-url"] || "",
    keypairPath: config["keypair"] || "",
    programId: config["program-id"] || "",
  };
}
