/**
 * deploy-and-init-svm.ts
 *
 * Initialize and configure a deployed SVM Bridge1024 program.
 * Called after `solana program deploy` has already deployed the program.
 *
 * Required environment variables:
 *   ADMIN_KEYPAIR_PATH - Path to admin keypair JSON file
 *   PROGRAM_ID         - Deployed program ID (base58)
 *   SVM_RPC_URL        - Solana RPC endpoint
 *   USDC_MINT          - USDC SPL token mint address (base58)
 *   PEER_CONTRACT      - EVM contract address (hex, with 0x prefix)
 *   SOURCE_CHAIN_ID    - SVM chain ID
 *   TARGET_CHAIN_ID    - EVM chain ID
 *   LIQUIDITY_AMOUNT   - Amount of tokens to add as liquidity (smallest unit)
 *   SKIP_LIQUIDITY     - "true" to skip liquidity step
 *   RELAYERS_FILE      - Path to relayers.json
 *   IDL_PATH           - Path to bridge1024 IDL JSON file
 *
 * Optional environment variables:
 *   RELAYER_MIN_BALANCE - Min balance (lamports) before funding (default: 50000000 = 0.05 SOL)
 *   RELAYER_FUND_AMOUNT - Amount (lamports) to send when below threshold (default: 500000000 = 0.5 SOL)
 */

import * as fs from "fs";
import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, Wallet } from "@coral-xyz/anchor";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  sendAndConfirmTransaction,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
  getAssociatedTokenAddress,
  getOrCreateAssociatedTokenAccount,
  createAccount as createTokenAccount,
} from "@solana/spl-token";
import BN from "bn.js";

// ---- Load environment variables ----

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return value;
}

const ADMIN_KEYPAIR_PATH = requireEnv("ADMIN_KEYPAIR_PATH");
const PROGRAM_ID = requireEnv("PROGRAM_ID");
const SVM_RPC_URL = requireEnv("SVM_RPC_URL");
const USDC_MINT = requireEnv("USDC_MINT");
const PEER_CONTRACT = requireEnv("PEER_CONTRACT");
const SOURCE_CHAIN_ID = requireEnv("SOURCE_CHAIN_ID");
const TARGET_CHAIN_ID = requireEnv("TARGET_CHAIN_ID");
const LIQUIDITY_AMOUNT = requireEnv("LIQUIDITY_AMOUNT");
const SKIP_LIQUIDITY = process.env.SKIP_LIQUIDITY === "true";
const RELAYERS_FILE = requireEnv("RELAYERS_FILE");
const IDL_PATH = requireEnv("IDL_PATH");

// ---- Helper functions ----

function loadKeypair(path: string): Keypair {
  const raw = JSON.parse(fs.readFileSync(path, "utf-8"));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}

interface RelayerConfig {
  name: string;
  evm_address: string;
  svm_pubkey: string;
}

interface RelayersFile {
  relayers: RelayerConfig[];
}

function loadRelayers(path: string): RelayersFile {
  return JSON.parse(fs.readFileSync(path, "utf-8"));
}

// ---- Main deployment logic ----

async function main() {
  console.log("============================================");
  console.log("  SVM Bridge1024 - Initialize & Configure");
  console.log("============================================");
  console.log(`Program ID:     ${PROGRAM_ID}`);
  console.log(`USDC Mint:      ${USDC_MINT}`);
  console.log(`Peer (EVM):     ${PEER_CONTRACT}`);
  console.log(`Source Chain:   ${SOURCE_CHAIN_ID}`);
  console.log(`Target Chain:   ${TARGET_CHAIN_ID}`);
  console.log(`Liquidity:      ${LIQUIDITY_AMOUNT}`);
  console.log(`Skip Liquidity: ${SKIP_LIQUIDITY}`);
  console.log("");

  // Load admin keypair
  const adminKeypair = loadKeypair(ADMIN_KEYPAIR_PATH);
  console.log(`Admin pubkey: ${adminKeypair.publicKey.toBase58()}`);

  // Setup connection and provider
  const connection = new Connection(SVM_RPC_URL, "confirmed");
  const wallet = new Wallet(adminKeypair);
  const provider = new AnchorProvider(connection, wallet, {
    commitment: "confirmed",
    preflightCommitment: "confirmed",
  });
  anchor.setProvider(provider);

  // Load IDL and create program
  const idl = JSON.parse(fs.readFileSync(IDL_PATH, "utf-8"));
  const programId = new PublicKey(PROGRAM_ID);

  // Update IDL with the new program ID (generated fresh each deployment)
  if (idl.address) {
    idl.address = PROGRAM_ID;
  }
  if (idl.metadata && idl.metadata.address) {
    idl.metadata.address = PROGRAM_ID;
  }
  const program = new Program(idl, provider);

  // Derive PDA addresses
  const [senderState] = PublicKey.findProgramAddressSync(
    [Buffer.from("sender_state")],
    programId
  );
  const [receiverState] = PublicKey.findProgramAddressSync(
    [Buffer.from("receiver_state")],
    programId
  );
  const [vault] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    programId
  );

  const usdcMint = new PublicKey(USDC_MINT);

  // Detect which token program the mint belongs to (SPL Token or Token-2022)
  const mintAccountInfo = await connection.getAccountInfo(usdcMint);
  if (!mintAccountInfo) {
    throw new Error(`USDC mint account not found: ${USDC_MINT}`);
  }
  const tokenProgramId = mintAccountInfo.owner;
  const isToken2022 = tokenProgramId.equals(TOKEN_2022_PROGRAM_ID);
  console.log(`Token Program:      ${tokenProgramId.toBase58()} (${isToken2022 ? "Token-2022" : "SPL Token"})`);

  console.log(`Sender State PDA:   ${senderState.toBase58()}`);
  console.log(`Receiver State PDA: ${receiverState.toBase58()}`);
  console.log(`Vault PDA:          ${vault.toBase58()}`);
  console.log("");

  // ---- Step 1: Initialize ----
  console.log(">>> Step 1: Initialize program...");
  try {
    const tx = await program.methods
      .initialize()
      .accounts({
        admin: adminKeypair.publicKey,
        vault: vault,
        senderState: senderState,
        receiverState: receiverState,
        systemProgram: SystemProgram.programId,
      })
      .signers([adminKeypair])
      .rpc();
    console.log(`    Initialize tx: ${tx}`);
  } catch (err: any) {
    // If already initialized, continue (idempotent)
    if (err.message?.includes("already in use")) {
      console.log("    Program already initialized, skipping.");
    } else {
      throw err;
    }
  }

  // ---- Step 2: Configure USDC ----
  console.log(">>> Step 2: Configure USDC mint...");
  const tx2 = await program.methods
    .configureUsdc(usdcMint)
    .accounts({
      admin: adminKeypair.publicKey,
      senderState: senderState,
      receiverState: receiverState,
    })
    .signers([adminKeypair])
    .rpc();
  console.log(`    Configure USDC tx: ${tx2}`);

  // ---- Step 3: Configure peer contract ----
  console.log(">>> Step 3: Configure peer contract...");
  // Peer contract is the EVM contract address (hex string)
  // configure_peer takes (String, u64, u64)
  // For SVM side: source = SVM chain, target = EVM chain
  const sourceChainId = new BN(SOURCE_CHAIN_ID);
  const targetChainId = new BN(TARGET_CHAIN_ID);

  const tx3 = await program.methods
    .configurePeer(PEER_CONTRACT, sourceChainId, targetChainId)
    .accounts({
      admin: adminKeypair.publicKey,
      senderState: senderState,
      receiverState: receiverState,
    })
    .signers([adminKeypair])
    .rpc();
  console.log(`    Configure peer tx: ${tx3}`);
  console.log(`    Peer: ${PEER_CONTRACT}, chains: ${SOURCE_CHAIN_ID} <-> ${TARGET_CHAIN_ID}`);

  // ---- Step 4: Register relayers ----
  console.log(">>> Step 4: Register relayers...");
  const relayersConfig = loadRelayers(RELAYERS_FILE);

  for (const relayer of relayersConfig.relayers) {
    const relayerPubkey = new PublicKey(relayer.svm_pubkey);
    console.log(`    Adding relayer ${relayer.name} (${relayer.svm_pubkey})...`);

    try {
      const tx = await program.methods
        .addRelayer(relayerPubkey)
        .accounts({
          admin: adminKeypair.publicKey,
          receiverState: receiverState,
        })
        .signers([adminKeypair])
        .rpc();
      console.log(`    Relayer ${relayer.name} registered: ${tx}`);
    } catch (err: any) {
      if (err.message?.includes("RelayerAlreadyExists")) {
        console.log(`    Relayer ${relayer.name} already registered, skipping.`);
      } else {
        throw err;
      }
    }
  }

  // ---- Step 4.5: Fund relayers if balance is low ----
  const relayerMinBalance = parseInt(process.env.RELAYER_MIN_BALANCE || "5000000");   // 0.005 SOL
  const relayerFundAmount = parseInt(process.env.RELAYER_FUND_AMOUNT || "50000000");  // 0.05 SOL
  console.log(`>>> Step 4.5: Check & fund relayer balances...`);
  console.log(`    Threshold: ${relayerMinBalance} lamports, Fund amount: ${relayerFundAmount} lamports`);

  for (const relayer of relayersConfig.relayers) {
    const relayerPubkey = new PublicKey(relayer.svm_pubkey);
    const balance = await connection.getBalance(relayerPubkey);
    console.log(`    ${relayer.name} (${relayer.svm_pubkey}): balance=${balance} lamports`);

    if (balance < relayerMinBalance) {
      console.log(`    Balance below threshold, funding ${relayerFundAmount} lamports...`);
      const tx = new Transaction().add(
        SystemProgram.transfer({
          fromPubkey: adminKeypair.publicKey,
          toPubkey: relayerPubkey,
          lamports: relayerFundAmount,
        })
      );
      const sig = await sendAndConfirmTransaction(connection, tx, [adminKeypair]);
      console.log(`    Funded ${relayer.name}: ${sig}`);
    } else {
      console.log(`    Balance sufficient, skipping.`);
    }
  }

  // ---- Step 5: Create vault token account & add liquidity ----
  if (SKIP_LIQUIDITY) {
    console.log(">>> Step 5: Skipping liquidity (SKIP_LIQUIDITY=true)");
  } else {
    console.log(`>>> Step 5: Add liquidity (${LIQUIDITY_AMOUNT} tokens)...`);

    // Create vault token account if it doesn't exist
    // The vault is a PDA, so we create a regular token account owned by the vault PDA
    console.log("    Ensuring vault token account exists...");
    const vaultTokenKeypair = Keypair.generate();
    let vaultTokenAccount: PublicKey;

    try {
      // Try to get existing ATA for vault (pass tokenProgramId for Token-2022 support)
      const vaultAta = await getAssociatedTokenAddress(usdcMint, vault, true, tokenProgramId);
      const ataInfo = await connection.getAccountInfo(vaultAta);

      if (ataInfo) {
        vaultTokenAccount = vaultAta;
        console.log(`    Vault token account exists: ${vaultTokenAccount.toBase58()}`);
      } else {
        // Create ATA for vault PDA (allowOwnerOffCurve = true for PDAs)
        const ata = await getOrCreateAssociatedTokenAccount(
          connection,
          adminKeypair,       // payer
          usdcMint,           // mint
          vault,              // owner (PDA)
          true,               // allowOwnerOffCurve (required for PDAs)
          undefined,          // commitment
          undefined,          // confirmOptions
          tokenProgramId      // token program (SPL Token or Token-2022)
        );
        vaultTokenAccount = ata.address;
        console.log(`    Created vault token account: ${vaultTokenAccount.toBase58()}`);
      }
    } catch (err) {
      // Fallback: create regular token account with vault as owner
      console.log("    Creating regular token account for vault...");
      vaultTokenAccount = await createTokenAccount(
        connection,
        adminKeypair,           // payer
        usdcMint,               // mint
        vault,                  // owner (PDA)
        vaultTokenKeypair,      // keypair for the account
        undefined,              // confirmOptions
        tokenProgramId          // token program (SPL Token or Token-2022)
      );
      console.log(`    Created vault token account: ${vaultTokenAccount.toBase58()}`);
    }

    // Get admin's token account (pass tokenProgramId for Token-2022 support)
    const adminTokenAccount = await getAssociatedTokenAddress(
      usdcMint,
      adminKeypair.publicKey,
      false,                    // allowOwnerOffCurve
      tokenProgramId            // token program
    );
    console.log(`    Admin token account: ${adminTokenAccount.toBase58()}`);

    // Add liquidity
    const liquidityAmount = new BN(LIQUIDITY_AMOUNT);
    const tx5 = await program.methods
      .addLiquidity(liquidityAmount)
      .accounts({
        admin: adminKeypair.publicKey,
        receiverState: receiverState,
        vault: vault,
        usdcMint: usdcMint,
        adminTokenAccount: adminTokenAccount,
        vaultTokenAccount: vaultTokenAccount,
        tokenProgram: tokenProgramId,
      })
      .signers([adminKeypair])
      .rpc();
    console.log(`    Add liquidity tx: ${tx5}`);
    console.log(`    Added ${LIQUIDITY_AMOUNT} tokens to vault`);
  }

  // ---- Verification ----
  console.log("");
  console.log("=== SVM Deployment Verification ===");
  console.log(`Program ID: ${PROGRAM_ID}`);

  const senderStateData = await (program.account as any)["senderState"].fetch(senderState);
  const receiverStateData = await (program.account as any)["receiverState"].fetch(receiverState);

  console.log(`Admin: ${senderStateData.admin.toBase58()}`);
  console.log(`Vault: ${senderStateData.vault.toBase58()}`);
  console.log(`USDC Mint: ${senderStateData.usdcMint.toBase58()}`);
  console.log(`Source Chain ID: ${senderStateData.sourceChainId.toString()}`);
  console.log(`Target Chain ID: ${senderStateData.targetChainId.toString()}`);
  console.log(`Relayer Count: ${receiverStateData.relayerCount.toString()}`);
  console.log(`Relayers: ${receiverStateData.relayers.map((r: any) => r.toBase58()).join(", ")}`);

  console.log("");
  console.log("SVM initialization complete!");
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error("SVM deployment failed:", err);
    process.exit(1);
  });
