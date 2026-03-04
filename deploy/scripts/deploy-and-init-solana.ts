/**
 * deploy-and-init-solana.ts
 *
 * Initialize and configure a deployed Solana Bridge1024 program.
 * Called after `anchor deploy` has already deployed the program.
 *
 * The Solana bridge now has both sender and receiver functionality,
 * mirroring the SVM (1024chain) bridge architecture:
 *   - Sender: Solana → 1024chain (stake)
 *   - Receiver: 1024chain → Solana (submit_signature + unlock)
 *
 * Required environment variables:
 *   ADMIN_KEYPAIR_PATH  - Path to admin keypair JSON file
 *   PROGRAM_ID          - Deployed program ID (base58)
 *   SOLANA_RPC_URL      - Solana RPC endpoint
 *   USDC_MINT           - USDC SPL token mint address (base58)
 *   PEER_CONTRACT       - Peer program ID on 1024chain (base58)
 *   SOURCE_CHAIN_ID     - Solana chain ID (e.g. 103 for devnet)
 *   TARGET_CHAIN_ID     - 1024chain ID (e.g. 91024)
 *   LIQUIDITY_AMOUNT    - Amount of tokens to add as liquidity (smallest unit)
 *   SKIP_LIQUIDITY      - "true" to skip liquidity step
 *   RELAYERS_FILE       - Path to relayers.json
 *   IDL_PATH            - Path to bridge1024_solana IDL JSON file
 *
 * Optional environment variables:
 *   RELAYER_MIN_BALANCE  - Min balance (lamports) before funding (default: 5000000 = 0.005 SOL)
 *   RELAYER_FUND_AMOUNT  - Amount (lamports) to send when below threshold (default: 50000000 = 0.05 SOL)
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
const SOLANA_RPC_URL = requireEnv("SOLANA_RPC_URL");
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
  console.log("=================================================");
  console.log("  Solana Bridge1024 - Initialize & Configure");
  console.log("=================================================");
  console.log(`Program ID:     ${PROGRAM_ID}`);
  console.log(`USDC Mint:      ${USDC_MINT}`);
  console.log(`Peer (SVM):     ${PEER_CONTRACT}`);
  console.log(`Source Chain:   ${SOURCE_CHAIN_ID} (Solana)`);
  console.log(`Target Chain:   ${TARGET_CHAIN_ID} (1024chain)`);
  console.log(`Liquidity:      ${LIQUIDITY_AMOUNT}`);
  console.log(`Skip Liquidity: ${SKIP_LIQUIDITY}`);
  console.log("");

  const adminKeypair = loadKeypair(ADMIN_KEYPAIR_PATH);
  console.log(`Admin pubkey: ${adminKeypair.publicKey.toBase58()}`);

  const connection = new Connection(SOLANA_RPC_URL, "confirmed");
  const wallet = new Wallet(adminKeypair);
  const provider = new AnchorProvider(connection, wallet, {
    commitment: "confirmed",
    preflightCommitment: "confirmed",
  });
  anchor.setProvider(provider);

  const idl = JSON.parse(fs.readFileSync(IDL_PATH, "utf-8"));
  const programId = new PublicKey(PROGRAM_ID);

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
        senderState: senderState,
        receiverState: receiverState,
        admin: adminKeypair.publicKey,
        vault: vault,
        systemProgram: SystemProgram.programId,
      })
      .signers([adminKeypair])
      .rpc();
    console.log(`    Initialize tx: ${tx}`);
  } catch (err: any) {
    const msg = err.message || String(err);
    if (
      msg.includes("already in use") ||
      msg.includes("already been processed") ||
      msg.includes("custom program error: 0x0")
    ) {
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
      senderState: senderState,
      receiverState: receiverState,
      admin: adminKeypair.publicKey,
    })
    .signers([adminKeypair])
    .rpc();
  console.log(`    Configure USDC tx: ${tx2}`);

  // ---- Step 3: Configure peer contract ----
  console.log(">>> Step 3: Configure peer contract...");
  // Solana's configure_peer takes (Pubkey, u64, u64)
  // It sets both sender and receiver state:
  //   sender: target_contract = peer, source = Solana, target = 1024chain
  //   receiver: source_contract = peer, source = 1024chain, target = Solana (swapped)
  const peerContractPubkey = new PublicKey(PEER_CONTRACT);
  const sourceChainId = new BN(SOURCE_CHAIN_ID);
  const targetChainId = new BN(TARGET_CHAIN_ID);

  const tx3 = await program.methods
    .configurePeer(peerContractPubkey, sourceChainId, targetChainId)
    .accounts({
      senderState: senderState,
      receiverState: receiverState,
      admin: adminKeypair.publicKey,
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
          receiverState: receiverState,
          admin: adminKeypair.publicKey,
          systemProgram: SystemProgram.programId,
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

    console.log("    Ensuring vault token account exists...");
    const vaultTokenKeypair = Keypair.generate();
    let vaultTokenAccount: PublicKey;

    try {
      const vaultAta = await getAssociatedTokenAddress(usdcMint, vault, true, tokenProgramId);
      const ataInfo = await connection.getAccountInfo(vaultAta);

      if (ataInfo) {
        vaultTokenAccount = vaultAta;
        console.log(`    Vault token account exists: ${vaultTokenAccount.toBase58()}`);
      } else {
        const ata = await getOrCreateAssociatedTokenAccount(
          connection,
          adminKeypair,
          usdcMint,
          vault,
          true,
          undefined,
          undefined,
          tokenProgramId
        );
        vaultTokenAccount = ata.address;
        console.log(`    Created vault token account: ${vaultTokenAccount.toBase58()}`);
      }
    } catch (err) {
      console.log("    Creating regular token account for vault...");
      vaultTokenAccount = await createTokenAccount(
        connection,
        adminKeypair,
        usdcMint,
        vault,
        vaultTokenKeypair,
        undefined,
        tokenProgramId
      );
      console.log(`    Created vault token account: ${vaultTokenAccount.toBase58()}`);
    }

    const adminTokenAccount = await getAssociatedTokenAddress(
      usdcMint,
      adminKeypair.publicKey,
      false,
      tokenProgramId
    );
    console.log(`    Admin token account: ${adminTokenAccount.toBase58()}`);

    const liquidityAmount = new BN(LIQUIDITY_AMOUNT);
    const tx5 = await program.methods
      .addLiquidity(liquidityAmount)
      .accounts({
        senderState: senderState,
        admin: adminKeypair.publicKey,
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
  console.log("=== Solana Deployment Verification ===");
  console.log(`Program ID: ${PROGRAM_ID}`);

  const senderStateData = await (program.account as any)["senderState"].fetch(senderState);
  const receiverStateData = await (program.account as any)["receiverState"].fetch(receiverState);

  console.log(`Admin:           ${senderStateData.admin.toBase58()}`);
  console.log(`Vault:           ${senderStateData.vault.toBase58()}`);
  console.log(`USDC Mint:       ${senderStateData.usdcMint.toBase58()}`);
  console.log(`Sender Nonce:    ${senderStateData.nonce.toString()}`);
  console.log(`Target Contract: ${senderStateData.targetContract}`);
  console.log(`Source Chain ID: ${senderStateData.sourceChainId.toString()}`);
  console.log(`Target Chain ID: ${senderStateData.targetChainId.toString()}`);
  console.log(`Relayer Count:   ${receiverStateData.relayerCount.toString()}`);
  console.log(`Relayers:        ${receiverStateData.relayers.map((r: any) => r.toBase58()).join(", ")}`);
  console.log(`Last Nonce:      ${receiverStateData.lastNonce.toString()}`);

  console.log("");
  console.log("Solana bridge initialization complete!");
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error("Solana deployment failed:", err);
    process.exit(1);
  });
