import * as fs from "fs";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import { createHash } from "crypto";

const PROGRAM_ID = process.env.PROGRAM_ID || "7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E";
const SVM_RPC_URL = process.env.SVM_RPC_URL || "https://rpc.1024chain.com";
const ADMIN_KEYPAIR_PATH = process.env.ADMIN_KEYPAIR_PATH || "../keys/admin-solana-keypair.json";
const FEE = parseInt(process.env.FEE || "1000000"); // 1 USDC = 1_000_000 (6 decimals)

function loadKeypair(path: string): Keypair {
  const raw = JSON.parse(fs.readFileSync(path, "utf-8"));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}

async function main() {
  const programId = new PublicKey(PROGRAM_ID);
  const adminKeypair = loadKeypair(ADMIN_KEYPAIR_PATH);
  const connection = new Connection(SVM_RPC_URL, "confirmed");

  console.log(`Program ID:     ${PROGRAM_ID}`);
  console.log(`RPC:            ${SVM_RPC_URL}`);
  console.log(`Admin:          ${adminKeypair.publicKey.toBase58()}`);
  console.log(`Fee:            ${FEE} (${FEE / 1e6} USDC)`);

  const [receiverState] = PublicKey.findProgramAddressSync(
    [Buffer.from("receiver_state")],
    programId
  );
  console.log(`Receiver State: ${receiverState.toBase58()}`);

  // Anchor discriminator: first 8 bytes of sha256("global:configure_fee")
  const discriminator = createHash("sha256")
    .update("global:configure_fee")
    .digest()
    .subarray(0, 8);

  // fee as u64 little-endian
  const feeBuffer = Buffer.alloc(8);
  feeBuffer.writeBigUInt64LE(BigInt(FEE));

  const data = Buffer.concat([discriminator, feeBuffer]);

  const ix = new TransactionInstruction({
    programId,
    keys: [
      { pubkey: receiverState, isSigner: false, isWritable: true },
      { pubkey: adminKeypair.publicKey, isSigner: true, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data,
  });

  const { blockhash } = await connection.getLatestBlockhash("confirmed");
  const messageV0 = new TransactionMessage({
    payerKey: adminKeypair.publicKey,
    recentBlockhash: blockhash,
    instructions: [ix],
  }).compileToV0Message();

  const tx = new VersionedTransaction(messageV0);
  tx.sign([adminKeypair]);

  const sig = await connection.sendTransaction(tx, { skipPreflight: false });
  console.log(`\nTransaction sent: ${sig}`);

  const confirmation = await connection.confirmTransaction(sig, "confirmed");
  if (confirmation.value.err) {
    console.error("Transaction failed:", confirmation.value.err);
    process.exit(1);
  }

  console.log(`Fee configured successfully to ${FEE / 1e6} USDC!`);
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error("Failed:", err);
    process.exit(1);
  });
