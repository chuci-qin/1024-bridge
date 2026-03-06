/**
 * add-liquidity-svm.ts
 *
 * Add liquidity to a deployed SVM Bridge1024 program vault.
 *
 * Required environment variables:
 *   ADMIN_KEYPAIR_PATH - Path to admin keypair JSON file
 *   PROGRAM_ID         - Deployed program ID (base58)
 *   SVM_RPC_URL        - Solana RPC endpoint
 *   USDC_MINT          - USDC SPL token mint address (base58)
 *   LIQUIDITY_AMOUNT   - Amount of tokens to add (smallest unit, e.g. 5000000 = 5 USDC)
 *   IDL_PATH           - Path to bridge1024 IDL JSON file
 */

import * as fs from "fs";
import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import {
  TOKEN_2022_PROGRAM_ID,
  getAssociatedTokenAddress,
  getOrCreateAssociatedTokenAccount,
} from "@solana/spl-token";
import BN from "bn.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Missing required env: ${name}`);
  return value;
}

async function main() {
  const ADMIN_KEYPAIR_PATH = requireEnv("ADMIN_KEYPAIR_PATH");
  const PROGRAM_ID = requireEnv("PROGRAM_ID");
  const SVM_RPC_URL = requireEnv("SVM_RPC_URL");
  const USDC_MINT = requireEnv("USDC_MINT");
  const LIQUIDITY_AMOUNT = requireEnv("LIQUIDITY_AMOUNT");
  const IDL_PATH = requireEnv("IDL_PATH");

  const adminKeypair = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(ADMIN_KEYPAIR_PATH, "utf-8")))
  );
  console.log(`Admin:    ${adminKeypair.publicKey.toBase58()}`);
  console.log(`Program:  ${PROGRAM_ID}`);
  console.log(`Amount:   ${LIQUIDITY_AMOUNT}`);

  const connection = new Connection(SVM_RPC_URL, "confirmed");
  const wallet = new Wallet(adminKeypair);
  const provider = new AnchorProvider(connection, wallet, {
    commitment: "confirmed",
    preflightCommitment: "confirmed",
  });
  anchor.setProvider(provider);

  const programId = new PublicKey(PROGRAM_ID);
  const idl = JSON.parse(fs.readFileSync(IDL_PATH, "utf-8"));
  if (idl.address) idl.address = PROGRAM_ID;
  if (idl.metadata?.address) idl.metadata.address = PROGRAM_ID;
  const program = new Program(idl, provider);

  const usdcMint = new PublicKey(USDC_MINT);
  const [receiverState] = PublicKey.findProgramAddressSync(
    [Buffer.from("receiver_state")],
    programId
  );
  const [vault] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    programId
  );

  const mintAccountInfo = await connection.getAccountInfo(usdcMint);
  if (!mintAccountInfo) throw new Error(`Mint not found: ${USDC_MINT}`);
  const tokenProgramId = mintAccountInfo.owner;
  const isToken2022 = tokenProgramId.equals(TOKEN_2022_PROGRAM_ID);
  console.log(`Token program: ${isToken2022 ? "Token-2022" : "SPL Token"}`);

  const vaultAta = await getAssociatedTokenAddress(usdcMint, vault, true, tokenProgramId);
  const ataInfo = await connection.getAccountInfo(vaultAta);
  let vaultTokenAccount: PublicKey;

  if (ataInfo) {
    vaultTokenAccount = vaultAta;
    console.log(`Vault token account: ${vaultTokenAccount.toBase58()}`);
  } else {
    const ata = await getOrCreateAssociatedTokenAccount(
      connection, adminKeypair, usdcMint, vault, true,
      undefined, undefined, tokenProgramId
    );
    vaultTokenAccount = ata.address;
    console.log(`Created vault token account: ${vaultTokenAccount.toBase58()}`);
  }

  const adminTokenAccount = await getAssociatedTokenAddress(
    usdcMint, adminKeypair.publicKey, false, tokenProgramId
  );
  console.log(`Admin token account: ${adminTokenAccount.toBase58()}`);

  const tx = await program.methods
    .addLiquidity(new BN(LIQUIDITY_AMOUNT))
    .accounts({
      admin: adminKeypair.publicKey,
      receiverState,
      vault,
      usdcMint,
      adminTokenAccount,
      vaultTokenAccount,
      tokenProgram: tokenProgramId,
    })
    .signers([adminKeypair])
    .rpc();

  console.log(`Add liquidity tx: ${tx}`);
  console.log(`Added ${LIQUIDITY_AMOUNT} tokens to vault`);
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error("Failed:", err);
    process.exit(1);
  });
