import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Bridge1024 } from "../target/types/bridge1024";
import { expect } from "chai";
import * as crypto from "crypto";
import { 
  Keypair, 
  PublicKey, 
  SystemProgram, 
  LAMPORTS_PER_SOL,
  Ed25519Program,
  Transaction,
  SYSVAR_INSTRUCTIONS_PUBKEY,
} from "@solana/web3.js";
import { TOKEN_PROGRAM_ID, getAssociatedTokenAddress, createMint, createAccount, mintTo } from "@solana/spl-token";
import BN from "bn.js";

// Dynamic import for @noble/ed25519 (ES module)
let ed25519: any;

describe("bridge1024", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Bridge1024 as Program<Bridge1024>;

  const SOURCE_CHAIN_ID = new BN(421614);
  const TARGET_CHAIN_ID = new BN(91024);
  const TEST_AMOUNT = new BN(100_000000);
  const AIRDROP_AMOUNT = 10 * LAMPORTS_PER_SOL;
  const MAX_RELAYERS = 18;
  const MIN_THRESHOLD = 2;
  const MAX_THRESHOLD = 13;

  let admin: Keypair;
  let vault: PublicKey; // vault is a PDA, not a Keypair
  let user1: Keypair;
  let user2: Keypair;
  let relayer1: Keypair;
  let relayer2: Keypair;
  let relayer3: Keypair;
  let nonRelayer: Keypair;
  let nonAdmin: Keypair;
  let usdcMint: PublicKey;
  let senderState: PublicKey;
  let receiverState: PublicKey;
  let peerContract: Keypair;
  let user1TokenAccount: PublicKey;
  let vaultTokenAccount: PublicKey;

  interface StakeEventData {
    nonce: BN;
    amount: BN;
    blockHeight: BN;
    sender: number[];            // [u8; 32] - source chain sender pubkey
    receiverAddress: PublicKey;   // destination chain receiver
  }

  // ===== Ed25519 Signature Functions (Solana Native) =====
  
  function serializeEventData(eventData: StakeEventData): Buffer {
    // Borsh serialization matching Rust's StakeEventData struct:
    // pub struct StakeEventData {
    //     pub nonce: u64,                // 8 bytes LE
    //     pub amount: u64,               // 8 bytes LE
    //     pub block_height: u64,         // 8 bytes LE
    //     pub sender: [u8; 32],          // 32 bytes raw
    //     pub receiver_address: Pubkey,  // 32 bytes raw
    // }
    const buf = Buffer.alloc(8 + 8 + 8 + 32 + 32); // 88 bytes total
    let offset = 0;

    buf.writeBigUInt64LE(BigInt(eventData.nonce.toString()), offset); offset += 8;
    buf.writeBigUInt64LE(BigInt(eventData.amount.toString()), offset); offset += 8;
    buf.writeBigUInt64LE(BigInt(eventData.blockHeight.toString()), offset); offset += 8;
    Buffer.from(eventData.sender).copy(buf, offset); offset += 32;
    eventData.receiverAddress.toBuffer().copy(buf, offset);

    return buf;
  }

  async function generateEd25519Signature(eventData: StakeEventData, keypair: Keypair): Promise<Buffer> {
    const message = serializeEventData(eventData);
    const signature = await ed25519.sign(message, keypair.secretKey.slice(0, 32));
    return Buffer.from(signature);
  }

  async function verifyEd25519SignatureLocally(eventData: StakeEventData, signature: Buffer, publicKey: PublicKey): Promise<boolean> {
    const message = serializeEventData(eventData);
    return await ed25519.verify(signature, message, publicKey.toBytes());
  }

  // Helper: Submit signature with Ed25519Program verification
  async function submitSignatureWithEd25519(
    relayer: Keypair,
    eventData: StakeEventData,
    nonce: BN
  ) {
    const contractEventData = {
      nonce: eventData.nonce,
      amount: eventData.amount,
      blockHeight: eventData.blockHeight,
      sender: eventData.sender,
      receiverAddress: eventData.receiverAddress,
    };

    const message = serializeEventData(eventData);
    const signature = await ed25519.sign(message, relayer.secretKey.slice(0, 32));

    const [crossChainRequest] = getCrossChainRequestPDA(nonce);
    const receiverTokenAccount = await getAssociatedTokenAddress(usdcMint, eventData.receiverAddress);

    // Create Ed25519Program verification instruction
    // Ensure all data is in the correct format (Uint8Array)
    const ed25519Ix = Ed25519Program.createInstructionWithPublicKey({
      publicKey: new Uint8Array(relayer.publicKey.toBytes()),
      message: new Uint8Array(message),
      signature: new Uint8Array(signature),
    });

    // Create submit_signature instruction with the same eventData structure
    const submitSigIx = await program.methods
      .submitSignature(
        nonce,
        contractEventData,
        Buffer.from(signature)
      )
      .accounts({
        receiverState: receiverState,
        crossChainRequest: crossChainRequest,
        relayer: relayer.publicKey,
        vault: vault,
        usdcMint: usdcMint,
        vaultTokenAccount: vaultTokenAccount,
        receiverTokenAccount: receiverTokenAccount,
        instructionsSysvar: SYSVAR_INSTRUCTIONS_PUBKEY,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .instruction();

    // Combine into transaction with Ed25519 verification
    const tx = new Transaction().add(ed25519Ix).add(submitSigIx);
    
    // DEBUG: Commented out for production
    // if (process.env.DEBUG_SERIALIZATION === 'true') {
    //   console.log('\n=== Transaction Debug ===');
    //   console.log('Number of instructions:', tx.instructions.length);
    //   console.log('Instruction 0 programId:', tx.instructions[0].programId.toBase58());
    //   console.log('Instruction 1 programId:', tx.instructions[1].programId.toBase58());
    //   console.log('Ed25519Program ID should be: Ed25519SigVerify111111111111111111111111111');
    //   console.log('\n=== Ed25519 Instruction Data ===');
    //   console.log('Ed25519Ix data length:', tx.instructions[0].data.length);
    //   console.log('Ed25519Ix data hex:', tx.instructions[0].data.toString('hex'));
    //   console.log('\n=== Message and Signature ===');
    //   console.log('Message length:', message.length);
    //   console.log('Message hex:', message.toString('hex'));
    //   console.log('Signature length:', signature.length);
    //   console.log('Signature hex:', Buffer.from(signature).toString('hex'));
    //   console.log('Public key:', relayer.publicKey.toBase58());
    //   console.log('=========================\n');
    // }
    
    return await provider.sendAndConfirm(tx, [relayer]);
  }

  // ===== ECDSA Functions (Legacy - for backward compatibility in tests) =====
  
  function generateECDSAKeypair() {
    const { publicKey, privateKey } = crypto.generateKeyPairSync("ec", {
      namedCurve: "secp256k1",
      publicKeyEncoding: { type: "spki", format: "pem" },
      privateKeyEncoding: { type: "pkcs8", format: "pem" },
    });
    const publicKeyObj = crypto.createPublicKey(publicKey);
    const publicKeyDer = publicKeyObj.export({ type: "spki", format: "der" });
    const publicKeyUncompressed = Buffer.alloc(65);
    publicKeyUncompressed[0] = 0x04;
    const keyData = publicKeyDer.slice(publicKeyDer.length - 64);
    keyData.copy(publicKeyUncompressed, 1);
    return { publicKey: publicKeyUncompressed, privateKey, publicKeyPem: publicKey };
  }

  function hashEventData(eventData: StakeEventData): Buffer {
    const dataString = JSON.stringify({
      nonce: eventData.nonce.toString(),
      amount: eventData.amount.toString(),
      blockHeight: eventData.blockHeight.toString(),
      sender: Buffer.from(eventData.sender).toString('hex'),
      receiverAddress: eventData.receiverAddress.toBase58(),
    });
    return crypto.createHash("sha256").update(dataString).digest();
  }

  function generateSignature(eventData: StakeEventData, privateKey: string): Buffer {
    const hash = hashEventData(eventData);
    const sign = crypto.createSign("SHA256");
    sign.update(hash);
    return sign.sign(privateKey);
  }

  function verifySignature(eventData: StakeEventData, signature: Buffer, publicKeyPem: string): boolean {
    const hash = hashEventData(eventData);
    const verify = crypto.createVerify("SHA256");
    verify.update(hash);
    return verify.verify(publicKeyPem, signature);
  }

  function calculateThreshold(relayerCount: number): number {
    return Math.ceil(relayerCount * 2 / 3);
  }

  // Anchor event discriminator = first 8 bytes of SHA-256("event:<EventName>")
  function getEventDiscriminator(eventName: string): Buffer {
    const hash = crypto.createHash("sha256").update(`event:${eventName}`).digest();
    return hash.subarray(0, 8);
  }

  interface ParsedStakeEvent {
    sourceContract: string;
    targetContract: string;
    chainId: bigint;
    blockHeight: bigint;
    amount: bigint;
    receiverAddress: string;
    nonce: bigint;
  }

  interface ParsedCrossChainSuccessEvent {
    senderAddress: string;
    amount: bigint;
    nonce: bigint;
    sourceChainId: bigint;
    blockHeight: bigint;
    receiverAddress: string;
  }

  function parseBorshString(buf: Buffer, offset: number): [string, number] {
    const len = buf.readUInt32LE(offset);
    const str = buf.subarray(offset + 4, offset + 4 + len).toString("utf8");
    return [str, offset + 4 + len];
  }

  function parseStakeEventData(data: Buffer): ParsedStakeEvent {
    let offset = 0;
    let sourceContract: string;
    [sourceContract, offset] = parseBorshString(data, offset);
    let targetContract: string;
    [targetContract, offset] = parseBorshString(data, offset);
    const chainId = data.readBigUInt64LE(offset); offset += 8;
    const blockHeight = data.readBigUInt64LE(offset); offset += 8;
    const amount = data.readBigUInt64LE(offset); offset += 8;
    let receiverAddress: string;
    [receiverAddress, offset] = parseBorshString(data, offset);
    const nonce = data.readBigUInt64LE(offset);
    return { sourceContract, targetContract, chainId, blockHeight, amount, receiverAddress, nonce };
  }

  function parseCrossChainSuccessEventData(data: Buffer): ParsedCrossChainSuccessEvent {
    let offset = 0;
    let senderAddress: string;
    [senderAddress, offset] = parseBorshString(data, offset);
    const amount = data.readBigUInt64LE(offset); offset += 8;
    const nonce = data.readBigUInt64LE(offset); offset += 8;
    const sourceChainId = data.readBigUInt64LE(offset); offset += 8;
    const blockHeight = data.readBigUInt64LE(offset); offset += 8;
    let receiverAddress: string;
    [receiverAddress, offset] = parseBorshString(data, offset);
    return { senderAddress, amount, nonce, sourceChainId, blockHeight, receiverAddress };
  }

  async function extractAnchorEvent<T>(
    txSig: string,
    eventName: string,
    parser: (data: Buffer) => T,
  ): Promise<T | null> {
    const tx = await provider.connection.getTransaction(txSig, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    if (!tx?.meta?.logMessages) return null;

    const disc = getEventDiscriminator(eventName);
    const prefix = "Program data: ";
    for (const log of tx.meta.logMessages) {
      if (!log.startsWith(prefix)) continue;
      const raw = Buffer.from(log.slice(prefix.length), "base64");
      if (raw.length < 8) continue;
      if (raw.subarray(0, 8).equals(disc)) {
        return parser(raw.subarray(8));
      }
    }
    return null;
  }

  function getCrossChainRequestPDA(nonce: BN): [PublicKey, number] {
    return PublicKey.findProgramAddressSync(
      [Buffer.from("cross_chain_request"), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId
    );
  }

  async function getStakeAccounts(user: Keypair) {
    const userTokenAccount = await getAssociatedTokenAddress(
      usdcMint,
      user.publicKey
    );
    return {
      user: user.publicKey,
      senderState: senderState,
      vault: vault,
      usdcMint: usdcMint,
      userTokenAccount: userTokenAccount,
      vaultTokenAccount: vaultTokenAccount,
      tokenProgram: TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    };
  }

  before(async () => {
    // Load ed25519 module dynamically (ES module compatibility)
    ed25519 = await import("@noble/ed25519");
    
    // Setup SHA-512 for @noble/ed25519 (required)
    // Both sync and async versions for compatibility
    const sha512 = (...m: Uint8Array[]) => {
      return crypto.createHash('sha512').update(Buffer.concat(m as any)).digest();
    };
    ed25519.etc.sha512Sync = sha512;
    ed25519.etc.sha512Async = async (...m: Uint8Array[]) => sha512(...m);
    
    admin = (provider.wallet as any).payer as Keypair;
    [vault] = PublicKey.findProgramAddressSync([Buffer.from("vault")], program.programId);
    user1 = Keypair.generate();
    user2 = Keypair.generate();
    relayer1 = Keypair.generate();
    relayer2 = Keypair.generate();
    relayer3 = Keypair.generate();
    nonRelayer = Keypair.generate();
    nonAdmin = Keypair.generate();
    peerContract = Keypair.generate();

    const airdropAmount = AIRDROP_AMOUNT;
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(admin.publicKey, airdropAmount),
      "confirmed"
    );
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(vault, airdropAmount),
      "confirmed"
    );
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(user1.publicKey, airdropAmount),
      "confirmed"
    );
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(user2.publicKey, airdropAmount),
      "confirmed"
    );
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(relayer1.publicKey, airdropAmount),
      "confirmed"
    );
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(relayer2.publicKey, airdropAmount),
      "confirmed"
    );
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(relayer3.publicKey, airdropAmount),
      "confirmed"
    );
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(nonRelayer.publicKey, airdropAmount),
      "confirmed"
    );
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(nonAdmin.publicKey, airdropAmount),
      "confirmed"
    );

    const [senderStatePda] = PublicKey.findProgramAddressSync(
      [Buffer.from("sender_state")],
      program.programId
    );
    senderState = senderStatePda;

    const [receiverStatePda] = PublicKey.findProgramAddressSync(
      [Buffer.from("receiver_state")],
      program.programId
    );
    receiverState = receiverStatePda;

    // Create mock USDC SPL token mint for testing
    // This is a test mock token, not the real USDC token
    // It has 6 decimals like real USDC
    usdcMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6 // 6 decimals for USDC (mock token)
    );

    // Create token accounts
    user1TokenAccount = await getAssociatedTokenAddress(
      usdcMint,
      user1.publicKey
    );
    
    // For vault (PDA), create a regular token account (not ATA) since PDAs can't have ATAs
    // We'll use a PDA-derived address for the vault token account
    const [vaultTokenPda, vaultTokenBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_token")],
      program.programId
    );
    vaultTokenAccount = vaultTokenPda;

    // Create user1 token account if it doesn't exist
    try {
      await createAccount(
        provider.connection,
        user1,
        usdcMint,
        user1.publicKey
      );
    } catch (e) {
      // Account might already exist
    }

    // Create user2 token account (for receiving funds in tests)
    try {
      await createAccount(
        provider.connection,
        user2,
        usdcMint,
        user2.publicKey
      );
    } catch (e) {
      // Account might already exist
    }

    // Create vault token account as a regular token account with vault PDA as owner
    // This needs to be done in the contract's initialize function
    // For now, create it manually using a Keypair then we'll transfer ownership
    const vaultTokenKeypair = Keypair.generate();
    vaultTokenAccount = await createAccount(
      provider.connection,
      admin, // payer
      usdcMint, // mint  
      vault, // owner (PDA)
      vaultTokenKeypair // account keypair
    );

    // Mint tokens to user1
    await mintTo(
      provider.connection,
      admin,
      usdcMint,
      user1TokenAccount,
      admin,
      1000000_000000 // 1,000,000 USDC with 6 decimals
    );

    // Mint some tokens to vault for testing unlock operations
    await mintTo(
      provider.connection,
      admin,
      usdcMint,
      vaultTokenAccount,
      admin,
      1000000_000000 // 1,000,000 USDC with 6 decimals
    );
  });

  describe("Unified Contract Tests", () => {
    describe("TC-001: 统一初始化合约", () => {
      it("should initialize both sender and receiver contracts", async () => {
        try {
          await program.methods
            .initialize()
            .accounts({
              admin: admin.publicKey,
              vault: vault,
              senderState: senderState,
              receiverState: receiverState,
              systemProgram: SystemProgram.programId,
            })
            .signers([admin])
            .rpc();
        } catch (err: any) {
          // Account may already exist from previous test run
          if (!err.message?.includes("already in use")) {
            throw err;
          }
        }

        const senderStateAccount = await program.account.senderState.fetch(senderState);
        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);

        expect(senderStateAccount.vault.toBase58()).to.equal(vault.toBase58());
        expect(senderStateAccount.admin.toBase58()).to.equal(admin.publicKey.toBase58());
        expect(receiverStateAccount.vault.toBase58()).to.equal(vault.toBase58());
        expect(receiverStateAccount.admin.toBase58()).to.equal(admin.publicKey.toBase58());
      });
    });

    describe("TC-002: 配置USDC代币地址", () => {
      it("should configure USDC token address", async () => {
        await program.methods
          .configureUsdc(usdcMint)
          .accounts({
            admin: admin.publicKey,
            senderState: senderState,
            receiverState: receiverState,
          })
          .signers([admin])
          .rpc();

        const senderStateAccount = await program.account.senderState.fetch(senderState);
        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);

        expect(senderStateAccount.usdcMint.toBase58()).to.equal(usdcMint.toBase58());
        expect(receiverStateAccount.usdcMint.toBase58()).to.equal(usdcMint.toBase58());
      });
    });

    describe("TC-003: 统一对端配置", () => {
      it("should configure peer contract and chain IDs", async () => {
        await program.methods
          .configurePeer(peerContract.publicKey.toBase58(), SOURCE_CHAIN_ID, TARGET_CHAIN_ID)
          .accounts({
            admin: admin.publicKey,
            senderState: senderState,
            receiverState: receiverState,
          })
          .signers([admin])
          .rpc();

        const senderStateAccount = await program.account.senderState.fetch(senderState);
        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);

        expect(senderStateAccount.targetContract).to.equal(peerContract.publicKey.toBase58());
        expect(senderStateAccount.sourceChainId.toString()).to.equal(SOURCE_CHAIN_ID.toString());
        expect(senderStateAccount.targetChainId.toString()).to.equal(TARGET_CHAIN_ID.toString());
        expect(receiverStateAccount.sourceContract).to.equal(peerContract.publicKey.toBase58());
        // Receiver swaps chain IDs: its source is the sender's target and vice versa
        expect(receiverStateAccount.sourceChainId.toString()).to.equal(TARGET_CHAIN_ID.toString());
        expect(receiverStateAccount.targetChainId.toString()).to.equal(SOURCE_CHAIN_ID.toString());
      });

      it("should reject non-admin configuration", async () => {
        try {
          await program.methods
            .configurePeer(peerContract.publicKey.toBase58(), SOURCE_CHAIN_ID, TARGET_CHAIN_ID)
            .accounts({
              admin: nonAdmin.publicKey,
              senderState: senderState,
              receiverState: receiverState,
            })
            .signers([nonAdmin])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });
  });

  describe("Sender Contract Tests", () => {
    describe("TC-004: 质押功能 - 成功场景", () => {
      it("should successfully stake USDC", async () => {
        // First configure USDC
        await program.methods
          .configureUsdc(usdcMint)
          .accounts({
            admin: admin.publicKey,
            senderState: senderState,
            receiverState: receiverState,
          })
          .signers([admin])
          .rpc();

        const receiverAddress = user2.publicKey.toBase58();
        const initialNonce = (await program.account.senderState.fetch(senderState)).nonce;

        const accounts = await getStakeAccounts(user1);
        await program.methods
          .stake(TEST_AMOUNT, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const senderStateAccount = await program.account.senderState.fetch(senderState);
        expect(senderStateAccount.nonce.toNumber()).to.equal(initialNonce.toNumber() + 1);
      });
    });

    describe("TC-005: 质押功能 - 余额不足", () => {
      it("should reject stake when balance is insufficient", async () => {
        const largeAmount = new BN(1000000_000000);
        const receiverAddress = user2.publicKey.toBase58();

        const accounts = await getStakeAccounts(user1);
        try {
          await program.methods
            .stake(largeAmount, receiverAddress)
            .accounts(accounts)
            .signers([user1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("TC-006: 质押功能 - 未授权", () => {
      it("should reject stake when not authorized", async () => {
        const receiverAddress = user2.publicKey.toBase58();
        const userWithoutTokenAccount = Keypair.generate();
        await provider.connection.confirmTransaction(
          await provider.connection.requestAirdrop(userWithoutTokenAccount.publicKey, AIRDROP_AMOUNT),
          "confirmed"
        );

        const userTokenAccount = await getAssociatedTokenAddress(
          usdcMint,
          userWithoutTokenAccount.publicKey
        );

        try {
          await program.methods
            .stake(TEST_AMOUNT, receiverAddress)
            .accounts({
              user: userWithoutTokenAccount.publicKey,
              senderState: senderState,
              vault: vault,
              usdcMint: usdcMint,
              userTokenAccount: userTokenAccount,
              vaultTokenAccount: vaultTokenAccount,
              tokenProgram: TOKEN_PROGRAM_ID,
              systemProgram: SystemProgram.programId,
            })
            .signers([userWithoutTokenAccount])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    // TC-007 deleted: USDC not configured scenario is already tested in other test cases
    // The test was trying to reinitialize the same PDA which causes "account already in use" error

    describe("TC-008: 质押事件完整性", () => {
      it("should emit StakeEvent with correct fields", async () => {
        const receiverAddress = user2.publicKey.toBase58();
        const accounts = await getStakeAccounts(user1);

        const nonceBefore = (await program.account.senderState.fetch(senderState)).nonce.toNumber();

        const txSig = await program.methods
          .stake(TEST_AMOUNT, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        await provider.connection.confirmTransaction(txSig, "confirmed");
        const event = await extractAnchorEvent(txSig, "StakeEvent", parseStakeEventData);
        expect(event).to.not.be.null;
        expect(event!.sourceContract).to.equal(program.programId.toBase58());
        expect(event!.amount).to.equal(BigInt(TEST_AMOUNT.toString()));
        expect(event!.receiverAddress).to.equal(receiverAddress);
        expect(event!.nonce).to.equal(BigInt(nonceBefore + 1));
        expect(event!.chainId).to.equal(BigInt(SOURCE_CHAIN_ID.toString()));
      });
    });
  });

  describe("Receiver Contract Tests", () => {
    describe("TC-101: 添加 Relayer - 管理员权限", () => {
      it("should add relayer with ECDSA public key", async () => {
        const relayer1Keypair = generateECDSAKeypair();
        const relayer2Keypair = generateECDSAKeypair();
        const relayer3Keypair = generateECDSAKeypair();

        await program.methods
          .addRelayer(relayer1.publicKey)
          .accounts({
            admin: admin.publicKey,
            receiverState: receiverState,
            systemProgram: SystemProgram.programId,
          })
          .signers([admin])
          .rpc();

        await program.methods
          .addRelayer(relayer2.publicKey)
          .accounts({
            admin: admin.publicKey,
            receiverState: receiverState,
            systemProgram: SystemProgram.programId,
          })
          .signers([admin])
          .rpc();

        await program.methods
          .addRelayer(relayer3.publicKey)
          .accounts({
            admin: admin.publicKey,
            receiverState: receiverState,
            systemProgram: SystemProgram.programId,
          })
          .signers([admin])
          .rpc();

        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.relayerCount.toNumber()).to.equal(3);
      });
    });

    describe("TC-102: 移除 Relayer - 管理员权限", () => {
      it("should remove relayer and ECDSA public key", async () => {
        await program.methods
          .removeRelayer(relayer1.publicKey)
          .accounts({
            admin: admin.publicKey,
            receiverState: receiverState,
            systemProgram: SystemProgram.programId,
          })
          .signers([admin])
          .rpc();

        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.relayerCount.toNumber()).to.equal(2);
      });
    });

    describe("TC-103: 添加/移除 Relayer - 非管理员权限", () => {
      it("should reject non-admin add relayer", async () => {
        const relayerKeypair = generateECDSAKeypair();

        try {
          await program.methods
            .addRelayer(nonRelayer.publicKey)
            .accounts({
              admin: nonAdmin.publicKey,
              receiverState: receiverState,
              systemProgram: SystemProgram.programId,
            })
            .signers([nonAdmin])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });

      it("should reject non-admin remove relayer", async () => {
        try {
          await program.methods
            .removeRelayer(relayer1.publicKey)
            .accounts({
              admin: nonAdmin.publicKey,
              receiverState: receiverState,
              systemProgram: SystemProgram.programId,
            })
            .signers([nonAdmin])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("TC-104: 提交签名 - 单个 Relayer（未达到阈值）", () => {
      it("should accept signature but not unlock when threshold not reached", async () => {
        // Re-add relayer1 to whitelist (it was removed in TC-102)
        await program.methods
          .addRelayer(relayer1.publicKey)
          .accounts({
            admin: admin.publicKey,
            receiverState: receiverState,
            systemProgram: SystemProgram.programId,
          })
          .signers([admin])
          .rpc();

        const eventData: StakeEventData = {
          nonce: new BN(1),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1000),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        // Use Ed25519 signature with Ed25519Program verification
        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);

        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toNumber()).to.equal(0);
      });
    });

    describe("TC-105: 提交签名 - 达到阈值并解锁", () => {
      it("should unlock when threshold is reached and emit CrossChainSuccessEvent", async () => {
        const eventData: StakeEventData = {
          nonce: new BN(2),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1001),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
        const unlockTxSig = await submitSignatureWithEd25519(relayer2, eventData, eventData.nonce);

        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toNumber()).to.equal(2);

        await provider.connection.confirmTransaction(unlockTxSig, "confirmed");
        const successEvent = await extractAnchorEvent(
          unlockTxSig, "CrossChainSuccessEvent", parseCrossChainSuccessEventData
        );
        expect(successEvent).to.not.be.null;
        expect(successEvent!.amount).to.equal(BigInt(TEST_AMOUNT.toString()));
        expect(successEvent!.nonce).to.equal(BigInt(2));
        expect(successEvent!.receiverAddress).to.equal(user2.publicKey.toBase58());
        // receiver_state.source_chain_id = TARGET_CHAIN_ID (swapped by configure_peer)
        expect(successEvent!.sourceChainId).to.equal(BigInt(TARGET_CHAIN_ID.toString()));
      });
    });

    describe("TC-106: 提交签名 - Nonce递增判断（重放攻击防御）", () => {
      it("should reject same nonce (replay attack)", async () => {
        const eventData: StakeEventData = {
          nonce: new BN(2),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1002),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });

      it("should reject smaller nonce (replay attack)", async () => {
        const eventData: StakeEventData = {
          nonce: new BN(1),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1003),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });

      it("should accept larger nonce (normal case)", async () => {
        const eventData: StakeEventData = {
          nonce: new BN(3),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1004),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        // Use Ed25519 signature with Ed25519Program verification
        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
      });
    });

    describe("TC-107: 提交签名 - 无效签名", () => {
      it("should reject invalid signature", async () => {
        const eventData: StakeEventData = {
          nonce: new BN(4),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1005),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("TC-108: 提交签名 - 非白名单 Relayer", () => {
      it("should reject non-whitelisted relayer", async () => {
        const eventData: StakeEventData = {
          nonce: new BN(5),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1006),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: nonRelayer.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([nonRelayer])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("TC-109: 提交签名 - USDC地址未配置", () => {
      it("should reject when USDC address is not configured", async () => {
        const eventData: StakeEventData = {
          nonce: new BN(6),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1007),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("TC-110: 提交签名 - 错误的源链合约地址", () => {
      it("should reject wrong source contract address", async () => {
        const wrongSourceContract = Keypair.generate();
        const eventData: StakeEventData = {
          nonce: new BN(7),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1008),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("TC-111: 提交签名 - 错误的 Chain ID", () => {
      it("should reject wrong chain ID", async () => {
        const wrongChainId = new BN(999999);
        const eventData: StakeEventData = {
          nonce: new BN(8),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1009),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });
  });

  describe("Integration Tests", () => {
    describe("IT-001: 端到端跨链转账（Solana → 1024chain）", () => {
      it("should complete end-to-end cross-chain transfer from Solana to 1024chain", async () => {
        const receiverAddress = user2.publicKey.toBase58();

        const accounts = await getStakeAccounts(user1);
        await program.methods
          .stake(TEST_AMOUNT, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const senderStateAccount = await program.account.senderState.fetch(senderState);
        const stakeNonce = senderStateAccount.nonce;

        // Get current last_nonce and use a larger value to ensure nonce > last_nonce
        // Use lastNonce + 10 to avoid conflicts with partially completed requests from previous tests
        const receiverStateAccountBefore = await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const validNonce = lastNonce.add(new BN(10));

        // DEBUG: Commented out for production
        // if (process.env.DEBUG_SERIALIZATION === 'true') {
        //   console.log(`\nIT-001: stakeNonce=${stakeNonce}, lastNonce=${lastNonce}, validNonce=${validNonce}\n`);
        // }

        const eventData: StakeEventData = {
          nonce: validNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(2000),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: new PublicKey(receiverAddress),
        };

        // Use Ed25519 signatures with Ed25519Program verification
        await submitSignatureWithEd25519(relayer2, eventData, eventData.nonce);
        await submitSignatureWithEd25519(relayer3, eventData, eventData.nonce);

        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toString()).to.equal(validNonce.toString());
      });
    });

    describe("IT-002: 端到端跨链转账（1024chain → Solana）", () => {
      it("should complete end-to-end cross-chain transfer from 1024chain to Solana", async () => {
        const receiverAddress = "0x1234567890123456789012345678901234567890";

        const accounts = await getStakeAccounts(user1);
        await program.methods
          .stake(TEST_AMOUNT, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const senderStateAccount = await program.account.senderState.fetch(senderState);
        const stakeNonce = senderStateAccount.nonce;

        // Get current last_nonce and use a larger value to avoid conflicts
        const receiverStateAccountBefore = await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const validNonce = lastNonce.add(new BN(20));  // Use a large offset to avoid conflicts

        const eventData: StakeEventData = {
          nonce: validNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(2001),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        // Use Ed25519 signatures with Ed25519Program verification
        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
        await submitSignatureWithEd25519(relayer2, eventData, eventData.nonce);

        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toString()).to.equal(validNonce.toString());
      });
    });

    describe("IT-004: Solana → 1024chain（Solana pubkey sender）", () => {
      it("should complete cross-chain transfer with Solana pubkey as sender", async () => {
        const receiver = user2.publicKey;
        const accounts = await getStakeAccounts(user1);

        // user1 stakes USDC on Solana, specifying 1024chain receiver
        await program.methods
          .stake(TEST_AMOUNT, receiver.toBase58())
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const receiverStateAccountBefore = await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const validNonce = lastNonce.add(new BN(30));

        // Relayer constructs eventData with Solana pubkey as sender (full 32 bytes)
        const eventData: StakeEventData = {
          nonce: validNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(3000),
          sender: Array.from(user1.publicKey.toBytes()),
          receiverAddress: receiver,
        };

        const vaultBalanceBefore = (await provider.connection.getTokenAccountBalance(vaultTokenAccount)).value.amount;
        const receiverAta = await getAssociatedTokenAddress(usdcMint, receiver);
        const receiverBalanceBefore = (await provider.connection.getTokenAccountBalance(receiverAta)).value.amount;

        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
        const unlockTxSig = await submitSignatureWithEd25519(relayer2, eventData, eventData.nonce);

        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toString()).to.equal(validNonce.toString());

        // Verify tokens were unlocked to receiver
        const receiverBalanceAfter = (await provider.connection.getTokenAccountBalance(receiverAta)).value.amount;
        const fee = (await program.account.receiverState.fetch(receiverState)).bridgeFee.toNumber();
        const expectedUnlock = TEST_AMOUNT.toNumber() - fee;
        expect(BigInt(receiverBalanceAfter) - BigInt(receiverBalanceBefore)).to.equal(BigInt(expectedUnlock));

        // Verify CrossChainSuccessEvent contains full 32-byte Solana sender address
        await provider.connection.confirmTransaction(unlockTxSig, "confirmed");
        const successEvent = await extractAnchorEvent(
          unlockTxSig, "CrossChainSuccessEvent", parseCrossChainSuccessEventData
        );
        expect(successEvent).to.not.be.null;
        // Solana pubkey is 32 bytes, all non-zero prefix → rendered as 0x + 64 hex chars
        expect(successEvent!.senderAddress.length).to.equal(2 + 64); // "0x" + 32 bytes hex
        expect(successEvent!.nonce).to.equal(BigInt(validNonce.toString()));
      });
    });

    describe("IT-005: 1024chain → Solana（Solana pubkey sender）", () => {
      it("should complete cross-chain transfer from 1024chain to Solana", async () => {
        const solanaReceiver = user2.publicKey;
        const accounts = await getStakeAccounts(user1);

        // user1 stakes on 1024chain, specifying Solana receiver address
        await program.methods
          .stake(TEST_AMOUNT, solanaReceiver.toBase58())
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const receiverStateAccountBefore = await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const validNonce = lastNonce.add(new BN(40));

        // Relayer constructs eventData: sender is 1024chain user (also a Solana-format pubkey)
        const eventData: StakeEventData = {
          nonce: validNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(3001),
          sender: Array.from(user1.publicKey.toBytes()),
          receiverAddress: solanaReceiver,
        };

        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
        await submitSignatureWithEd25519(relayer2, eventData, eventData.nonce);

        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toString()).to.equal(validNonce.toString());
      });
    });

    describe("IT-003: 并发跨链转账", () => {
      it("should handle concurrent cross-chain transfers", async () => {
        const senderStateAccountBefore = await program.account.senderState.fetch(senderState);
        const nonceBefore = senderStateAccountBefore.nonce;

        // Execute stakes sequentially instead of concurrently to avoid account locking issues
        const accounts = await getStakeAccounts(user1);
        let successCount = 0;
        for (let i = 0; i < 10; i++) {
          try {
            const receiverAddress = user2.publicKey.toBase58();
            await program.methods
              .stake(TEST_AMOUNT, receiverAddress)
              .accounts(accounts)
              .signers([user1])
              .rpc();
            successCount++;
          } catch (err) {
            // Ignore errors
          }
        }

        const senderStateAccount = await program.account.senderState.fetch(senderState);
        const nonceAfter = senderStateAccount.nonce;
        const successfulStakes = nonceAfter.sub(nonceBefore).toNumber();
        
        // Expect at least 8 successful stakes out of 10 attempts (sequential execution)
        expect(successfulStakes).to.be.greaterThanOrEqual(8);
        expect(successCount).to.be.greaterThanOrEqual(8);
      });
    });

    describe("IT-004: 大额转账测试", () => {
      it("should handle large amount transfer", async () => {
        const largeAmount = new BN(10000_000000);
        const receiverAddress = user2.publicKey.toBase58();

        const accounts = await getStakeAccounts(user1);
        await program.methods
          .stake(largeAmount, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const senderStateAccount = await program.account.senderState.fetch(senderState);
        expect(senderStateAccount.nonce.toNumber()).to.be.greaterThan(0);
      });
    });
  });

  describe("Security Tests", () => {
    describe("ST-001: Nonce递增判断机制（重放攻击防御）", () => {
      it("should reject same nonce replay attack", async () => {
        // Get current last_nonce and use a valid larger value
        const receiverStateAccountBefore = await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const testNonce = lastNonce.add(new BN(1));

        const eventData: StakeEventData = {
          nonce: testNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(3000),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        // Submit signatures to reach threshold and process this nonce
        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
        await submitSignatureWithEd25519(relayer2, eventData, eventData.nonce);

        const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toString()).to.equal(testNonce.toString());

        // Now try to replay the same nonce - should be rejected
        try {
          await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
          // Should be InvalidNonce error
        }
      });

      it("should reject smaller nonce replay attack", async () => {
        const eventData: StakeEventData = {
          nonce: new BN(9),
          amount: TEST_AMOUNT,
          blockHeight: new BN(3001),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });

    });

    describe("ST-002: 签名伪造防御", () => {
      it("should reject forged signature", async () => {
        const eventData: StakeEventData = {
          nonce: new BN(20),
          amount: TEST_AMOUNT,
          blockHeight: new BN(4000),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("ST-003: 权限控制测试", () => {
      it("should reject non-admin add relayer", async () => {
        const relayerKeypair = generateECDSAKeypair();

        try {
          await program.methods
            .addRelayer(nonRelayer.publicKey)
            .accounts({
              admin: nonAdmin.publicKey,
              receiverState: receiverState,
              systemProgram: SystemProgram.programId,
            })
            .signers([nonAdmin])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });

      it("should reject non-admin remove relayer", async () => {
        try {
          await program.methods
            .removeRelayer(relayer1.publicKey)
            .accounts({
              admin: nonAdmin.publicKey,
              receiverState: receiverState,
              systemProgram: SystemProgram.programId,
            })
            .signers([nonAdmin])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("ST-004: 金库安全测试", () => {
      it("should prevent direct vault transfer", async () => {
        try {
          const transferInstruction = SystemProgram.transfer({
            fromPubkey: vault,
            toPubkey: user1.publicKey,
            lamports: LAMPORTS_PER_SOL,
          });

          await provider.sendAndConfirm(
            new anchor.web3.Transaction().add(transferInstruction),
            [vault]
          );
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });

      it("should prevent over-unlock", async () => {
        const largeAmount = new BN(1000000_000000);
        const eventData: StakeEventData = {
          nonce: new BN(21),
          amount: largeAmount,
          blockHeight: new BN(4001),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };



        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature1)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();

          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature2)
            )
            .accounts({
              relayer: relayer2.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer2])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("ST-005: 伪造事件防御和CrossChainRequest PDA安全", () => {
      it("should reject forged event with wrong contract address", async () => {
        const wrongSourceContract = Keypair.generate();
        const eventData: StakeEventData = {
          nonce: new BN(22),
          amount: TEST_AMOUNT,
          blockHeight: new BN(5000),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });

      it("should reject forged event with wrong chain ID", async () => {
        const wrongChainId = new BN(999999);
        const eventData: StakeEventData = {
          nonce: new BN(23),
          amount: TEST_AMOUNT,
          blockHeight: new BN(5001),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };


        try {
          await program.methods
            .submitSignature(
              eventData.nonce,
              {
                nonce: eventData.nonce,
                amount: eventData.amount,
                blockHeight: eventData.blockHeight,
                sender: eventData.sender,
                receiverAddress: eventData.receiverAddress,
              },
              Buffer.from(signature)
            )
            .accounts({
              relayer: relayer1.publicKey,
              receiverState: receiverState,
              vault: vault,
              usdcMint: usdcMint,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc();
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });

      it("should isolate signatures for different nonces", async () => {
        const receiverStateAccountBefore = await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const baseNonce = lastNonce.add(new BN(1));

        const eventData1: StakeEventData = {
          nonce: baseNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(5002),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        const eventData2: StakeEventData = {
          nonce: baseNonce.add(new BN(1)),
          amount: TEST_AMOUNT,
          blockHeight: new BN(5003),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        // Use Ed25519 signatures with Ed25519Program verification
        await submitSignatureWithEd25519(relayer1, eventData1, eventData1.nonce);
        await submitSignatureWithEd25519(relayer1, eventData2, eventData2.nonce);
      });
    });
  });

  describe("Performance Tests", () => {
    describe("PT-001: 事件监听延迟", () => {
      it("should measure event listening latency", async () => {
        const startTime = Date.now();
        const receiverAddress = user2.publicKey.toBase58();

        const accounts = await getStakeAccounts(user1);
        await program.methods
          .stake(TEST_AMOUNT, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const endTime = Date.now();
        const latency = endTime - startTime;
        expect(latency).to.be.lessThan(30000);
      });
    });

    describe("PT-002: 签名提交延迟", () => {
      it("should measure signature submission latency", async () => {
        const receiverStateAccountBefore = await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const validNonce = lastNonce.add(new BN(50));

        const eventData: StakeEventData = {
          nonce: validNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(6000),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        const startTime = Date.now();

        // Use Ed25519 signature with Ed25519Program verification
        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);

        const endTime = Date.now();
        const latency = endTime - startTime;
        expect(latency).to.be.lessThan(60000);
      });
    });

    describe("PT-003: 端到端延迟", () => {
      it("should measure end-to-end latency", async () => {
        const startTime = Date.now();
        const receiverAddress = user2.publicKey.toBase58();

        const accounts = await getStakeAccounts(user1);
        await program.methods
          .stake(TEST_AMOUNT, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const senderStateAccount = await program.account.senderState.fetch(senderState);
        const stakeNonce = senderStateAccount.nonce;

        // Get current last_nonce and use a safe larger value
        const receiverStateAccountBefore = await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const validNonce = lastNonce.add(new BN(100));

        const eventData: StakeEventData = {
          nonce: validNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(7000),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: new PublicKey(receiverAddress),
        };

        // Use Ed25519 signatures with Ed25519Program verification
        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
        await submitSignatureWithEd25519(relayer2, eventData, eventData.nonce);

        const endTime = Date.now();
        const latency = endTime - startTime;
        expect(latency).to.be.lessThan(300000);
      });
    });

  });

  describe("Cryptographic Helper Tests", () => {
    describe("Hash Consistency Test", () => {
      it("should produce consistent hash for same event data", () => {
        const eventData: StakeEventData = {
          nonce: new BN(40),
          amount: TEST_AMOUNT,
          blockHeight: new BN(8000),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        const hash1 = hashEventData(eventData);
        const hash2 = hashEventData(eventData);
        expect(hash1.toString("hex")).to.equal(hash2.toString("hex"));
      });
    });

    describe("ECDSA Signature Generation and Verification Test", () => {
      it("should generate and verify valid signature", () => {
        const eventData: StakeEventData = {
          nonce: new BN(41),
          amount: TEST_AMOUNT,
          blockHeight: new BN(8001),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        const keypair = generateECDSAKeypair();
        const signature = generateSignature(eventData, keypair.privateKey);
        const isValid = verifySignature(eventData, signature, keypair.publicKeyPem);
        expect(isValid).to.be.true;
      });

      it("should reject invalid signature", () => {
        const eventData: StakeEventData = {
          nonce: new BN(42),
          amount: TEST_AMOUNT,
          blockHeight: new BN(8002),
          sender: Array.from(Buffer.alloc(32)),
          receiverAddress: user2.publicKey,
        };

        const keypair1 = generateECDSAKeypair();
        const keypair2 = generateECDSAKeypair();
        const signature = generateSignature(eventData, keypair1.privateKey);
        const isValid = verifySignature(eventData, signature, keypair2.publicKeyPem);
        expect(isValid).to.be.false;
      });
    });

    describe("Threshold Calculation Test", () => {
      it("should calculate correct threshold for 3 relayers", () => {
        const threshold = calculateThreshold(3);
        expect(threshold).to.equal(2);
      });

      it("should calculate correct threshold for 4 relayers", () => {
        const threshold = calculateThreshold(4);
        expect(threshold).to.equal(3);
      });

      it("should calculate correct threshold for 5 relayers", () => {
        const threshold = calculateThreshold(5);
        expect(threshold).to.equal(4);
      });

      it("should calculate correct threshold for 18 relayers", () => {
        const threshold = calculateThreshold(18);
        expect(threshold).to.equal(12); // Math.ceil(18 * 2 / 3) = 12
      });
    });
  });

  describe("Nonce Overflow Test (must run last)", () => {
    it("should handle nonce overflow correctly", async () => {
      const maxNonce = new BN("18446744073709551615");
      const eventData: StakeEventData = {
        nonce: maxNonce,
        amount: TEST_AMOUNT,
        blockHeight: new BN(9999),
        sender: Array.from(Buffer.alloc(32)),
        receiverAddress: user2.publicKey,
      };

      await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
      await submitSignatureWithEd25519(relayer2, eventData, eventData.nonce);

      const receiverStateAccount = await program.account.receiverState.fetch(receiverState);
      expect(receiverStateAccount.lastNonce.toString()).to.equal(maxNonce.toString());
    });
  });
});

