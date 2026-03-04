import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Bridge1024Solana } from "../target/types/bridge1024_solana";
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
import {
  TOKEN_PROGRAM_ID,
  getAssociatedTokenAddress,
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import BN from "bn.js";

let ed25519: any;

describe("bridge1024_solana", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Bridge1024Solana as Program<Bridge1024Solana>;

  const SOURCE_CHAIN_ID = new BN(103); // Solana devnet
  const TARGET_CHAIN_ID = new BN(91024); // 1024chain
  const TEST_AMOUNT = new BN(100_000000);
  const AIRDROP_AMOUNT = 10 * LAMPORTS_PER_SOL;
  const MAX_RELAYERS = 18;

  let admin: Keypair;
  let vault: PublicKey;
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

  // ===== StakeEventData interface for submit_signature =====
  // Matches Rust struct: { nonce: u64, amount: u64, block_height: u64, sender: [u8; 32], receiver_address: Pubkey }

  interface SubmitEventData {
    nonce: BN;
    amount: BN;
    blockHeight: BN;
    sender: number[];
    receiverAddress: PublicKey;
  }

  // ===== Ed25519 Signature Functions =====

  function serializeEventData(eventData: SubmitEventData): Buffer {
    const buf = Buffer.alloc(88);
    let offset = 0;
    buf.writeBigUInt64LE(BigInt(eventData.nonce.toString()), offset);
    offset += 8;
    buf.writeBigUInt64LE(BigInt(eventData.amount.toString()), offset);
    offset += 8;
    buf.writeBigUInt64LE(BigInt(eventData.blockHeight.toString()), offset);
    offset += 8;
    Buffer.from(eventData.sender).copy(buf, offset);
    offset += 32;
    eventData.receiverAddress.toBuffer().copy(buf, offset);
    return buf;
  }

  async function submitSignatureWithEd25519(
    relayer: Keypair,
    eventData: SubmitEventData,
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
    const signature = await ed25519.sign(
      message,
      relayer.secretKey.slice(0, 32)
    );

    const [crossChainRequest] = getCrossChainRequestPDA(nonce);
    const receiverTokenAccount = await getAssociatedTokenAddress(
      usdcMint,
      eventData.receiverAddress
    );

    const ed25519Ix = Ed25519Program.createInstructionWithPublicKey({
      publicKey: new Uint8Array(relayer.publicKey.toBytes()),
      message: new Uint8Array(message),
      signature: new Uint8Array(signature),
    });

    const submitSigIx = await program.methods
      .submitSignature(nonce, contractEventData, Buffer.from(signature))
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

    const tx = new Transaction().add(ed25519Ix).add(submitSigIx);

    return await provider.sendAndConfirm(tx, [relayer]);
  }

  function calculateThreshold(relayerCount: number): number {
    return Math.ceil((relayerCount * 2) / 3);
  }

  function getEventDiscriminator(eventName: string): Buffer {
    const hash = crypto
      .createHash("sha256")
      .update(`event:${eventName}`)
      .digest();
    return hash.subarray(0, 8);
  }

  interface ParsedStakeEvent {
    sourceContract: string;
    targetContract: string;
    chainId: bigint;
    blockHeight: bigint;
    amount: bigint;
    sender: string;
    receiverAddress: string;
    nonce: bigint;
  }

  interface ParsedCrossChainSuccessEvent {
    evmAddress: string;
    amount: bigint;
    nonce: bigint;
    sourceChainId: bigint;
    blockHeight: bigint;
    receiverAddress: string;
  }

  function parseBorshString(
    buf: Buffer,
    offset: number
  ): [string, number] {
    const len = buf.readUInt32LE(offset);
    const str = buf
      .subarray(offset + 4, offset + 4 + len)
      .toString("utf8");
    return [str, offset + 4 + len];
  }

  function parseStakeEventData(data: Buffer): ParsedStakeEvent {
    let offset = 0;
    let sourceContract: string;
    [sourceContract, offset] = parseBorshString(data, offset);
    let targetContract: string;
    [targetContract, offset] = parseBorshString(data, offset);
    const chainId = data.readBigUInt64LE(offset);
    offset += 8;
    const blockHeight = data.readBigUInt64LE(offset);
    offset += 8;
    const amount = data.readBigUInt64LE(offset);
    offset += 8;
    let sender: string;
    [sender, offset] = parseBorshString(data, offset);
    let receiverAddress: string;
    [receiverAddress, offset] = parseBorshString(data, offset);
    const nonce = data.readBigUInt64LE(offset);
    return {
      sourceContract,
      targetContract,
      chainId,
      blockHeight,
      amount,
      sender,
      receiverAddress,
      nonce,
    };
  }

  function parseCrossChainSuccessEventData(
    data: Buffer
  ): ParsedCrossChainSuccessEvent {
    let offset = 0;
    let evmAddress: string;
    [evmAddress, offset] = parseBorshString(data, offset);
    const amount = data.readBigUInt64LE(offset);
    offset += 8;
    const nonce = data.readBigUInt64LE(offset);
    offset += 8;
    const sourceChainId = data.readBigUInt64LE(offset);
    offset += 8;
    const blockHeight = data.readBigUInt64LE(offset);
    offset += 8;
    let receiverAddress: string;
    [receiverAddress, offset] = parseBorshString(data, offset);
    return {
      evmAddress,
      amount,
      nonce,
      sourceChainId,
      blockHeight,
      receiverAddress,
    };
  }

  async function extractAnchorEvent<T>(
    txSig: string,
    eventName: string,
    parser: (data: Buffer) => T
  ): Promise<T | null> {
    const disc = getEventDiscriminator(eventName);
    const prefix = "Program data: ";

    for (let attempt = 0; attempt < 5; attempt++) {
      const tx = await provider.connection.getTransaction(txSig, {
        commitment: "confirmed",
        maxSupportedTransactionVersion: 0,
      });
      if (tx?.meta?.logMessages) {
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
      await new Promise((r) => setTimeout(r, 500));
    }
    return null;
  }

  function getCrossChainRequestPDA(nonce: BN): [PublicKey, number] {
    return PublicKey.findProgramAddressSync(
      [
        Buffer.from("cross_chain_request"),
        nonce.toArrayLike(Buffer, "le", 8),
      ],
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
    ed25519 = await import("@noble/ed25519");

    const sha512 = (...m: Uint8Array[]) => {
      return crypto
        .createHash("sha512")
        .update(Buffer.concat(m as any))
        .digest();
    };
    ed25519.etc.sha512Sync = sha512;
    ed25519.etc.sha512Async = async (...m: Uint8Array[]) => sha512(...m);

    admin = Keypair.generate();
    [vault] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault")],
      program.programId
    );
    user1 = Keypair.generate();
    user2 = Keypair.generate();
    relayer1 = Keypair.generate();
    relayer2 = Keypair.generate();
    relayer3 = Keypair.generate();
    nonRelayer = Keypair.generate();
    nonAdmin = Keypair.generate();
    peerContract = Keypair.generate();

    const allKeys = [
      admin,
      user1,
      user2,
      relayer1,
      relayer2,
      relayer3,
      nonRelayer,
      nonAdmin,
    ];
    for (const kp of allKeys) {
      await provider.connection.confirmTransaction(
        await provider.connection.requestAirdrop(
          kp.publicKey,
          AIRDROP_AMOUNT
        ),
        "confirmed"
      );
    }
    await provider.connection.confirmTransaction(
      await provider.connection.requestAirdrop(vault, AIRDROP_AMOUNT),
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

    usdcMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    user1TokenAccount = await getAssociatedTokenAddress(
      usdcMint,
      user1.publicKey
    );

    try {
      await createAccount(
        provider.connection,
        user1,
        usdcMint,
        user1.publicKey
      );
    } catch (_) {}

    try {
      await createAccount(
        provider.connection,
        user2,
        usdcMint,
        user2.publicKey
      );
    } catch (_) {}

    const vaultTokenKeypair = Keypair.generate();
    vaultTokenAccount = await createAccount(
      provider.connection,
      admin,
      usdcMint,
      vault,
      vaultTokenKeypair
    );

    await mintTo(
      provider.connection,
      admin,
      usdcMint,
      user1TokenAccount,
      admin,
      1_000_000_000_000
    );

    await mintTo(
      provider.connection,
      admin,
      usdcMint,
      vaultTokenAccount,
      admin,
      1_000_000_000_000
    );
  });

  // ===================================================================
  // Unified Contract Tests (mirror SVM TC-001~TC-003)
  // ===================================================================

  describe("Unified Contract Tests", () => {
    describe("TC-001: Initialize both sender and receiver", () => {
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
          if (!err.message?.includes("already in use")) {
            throw err;
          }
        }

        const senderStateAccount =
          await program.account.senderState.fetch(senderState);
        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);

        expect(senderStateAccount.vault.toBase58()).to.equal(
          vault.toBase58()
        );
        expect(senderStateAccount.admin.toBase58()).to.equal(
          admin.publicKey.toBase58()
        );
        expect(receiverStateAccount.vault.toBase58()).to.equal(
          vault.toBase58()
        );
        expect(receiverStateAccount.admin.toBase58()).to.equal(
          admin.publicKey.toBase58()
        );
      });
    });

    describe("TC-002: Configure USDC on both states", () => {
      it("should configure USDC token address on both states", async () => {
        await program.methods
          .configureUsdc(usdcMint)
          .accounts({
            admin: admin.publicKey,
            senderState: senderState,
            receiverState: receiverState,
          })
          .signers([admin])
          .rpc();

        const senderStateAccount =
          await program.account.senderState.fetch(senderState);
        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);

        expect(senderStateAccount.usdcMint.toBase58()).to.equal(
          usdcMint.toBase58()
        );
        expect(receiverStateAccount.usdcMint.toBase58()).to.equal(
          usdcMint.toBase58()
        );
      });
    });

    describe("TC-003: Configure peer on both states", () => {
      it("should configure peer contract and chain IDs", async () => {
        await program.methods
          .configurePeer(
            peerContract.publicKey,
            SOURCE_CHAIN_ID,
            TARGET_CHAIN_ID
          )
          .accounts({
            admin: admin.publicKey,
            senderState: senderState,
            receiverState: receiverState,
          })
          .signers([admin])
          .rpc();

        const senderStateAccount =
          await program.account.senderState.fetch(senderState);
        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);

        expect(senderStateAccount.targetContract).to.equal(
          peerContract.publicKey.toBase58()
        );
        expect(senderStateAccount.sourceChainId.toString()).to.equal(
          SOURCE_CHAIN_ID.toString()
        );
        expect(senderStateAccount.targetChainId.toString()).to.equal(
          TARGET_CHAIN_ID.toString()
        );
        expect(receiverStateAccount.sourceContract).to.equal(
          peerContract.publicKey.toBase58()
        );
        expect(receiverStateAccount.sourceChainId.toString()).to.equal(
          TARGET_CHAIN_ID.toString()
        );
        expect(receiverStateAccount.targetChainId.toString()).to.equal(
          SOURCE_CHAIN_ID.toString()
        );
      });

      it("should reject non-admin configuration", async () => {
        try {
          await program.methods
            .configurePeer(
              peerContract.publicKey,
              SOURCE_CHAIN_ID,
              TARGET_CHAIN_ID
            )
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

  // ===================================================================
  // Sender Contract Tests (mirror SVM TC-004~TC-008)
  // ===================================================================

  describe("Sender Contract Tests", () => {
    describe("TC-004: Stake success (no fee)", () => {
      it("should successfully stake USDC with full amount", async () => {
        const receiverAddress = user2.publicKey.toBase58();
        const initialNonce = (
          await program.account.senderState.fetch(senderState)
        ).nonce;

        const userBefore = await getAccount(
          provider.connection,
          user1TokenAccount
        );
        const vaultBefore = await getAccount(
          provider.connection,
          vaultTokenAccount
        );

        const accounts = await getStakeAccounts(user1);
        await program.methods
          .stake(TEST_AMOUNT, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const senderStateAccount =
          await program.account.senderState.fetch(senderState);
        expect(senderStateAccount.nonce.toNumber()).to.equal(
          initialNonce.toNumber() + 1
        );

        const userAfter = await getAccount(
          provider.connection,
          user1TokenAccount
        );
        const vaultAfter = await getAccount(
          provider.connection,
          vaultTokenAccount
        );

        expect(
          Number(vaultAfter.amount - vaultBefore.amount)
        ).to.equal(TEST_AMOUNT.toNumber());
        expect(
          Number(userBefore.amount - userAfter.amount)
        ).to.equal(TEST_AMOUNT.toNumber());
      });
    });

    describe("TC-005: Stake insufficient balance", () => {
      it("should reject stake when balance is insufficient", async () => {
        const largeAmount = new BN(999_000_000_000_000);
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

    describe("TC-006: Stake unauthorized", () => {
      it("should reject stake when user has no token account", async () => {
        const userWithoutTokenAccount = Keypair.generate();
        await provider.connection.confirmTransaction(
          await provider.connection.requestAirdrop(
            userWithoutTokenAccount.publicKey,
            AIRDROP_AMOUNT
          ),
          "confirmed"
        );

        const userTokenAccount = await getAssociatedTokenAddress(
          usdcMint,
          userWithoutTokenAccount.publicKey
        );

        try {
          await program.methods
            .stake(
              TEST_AMOUNT,
              user2.publicKey.toBase58()
            )
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

    describe("TC-008: Stake event integrity", () => {
      it("should emit StakeEvent with correct fields", async () => {
        const receiverAddress = user2.publicKey.toBase58();
        const accounts = await getStakeAccounts(user1);

        const nonceBefore = (
          await program.account.senderState.fetch(senderState)
        ).nonce.toNumber();

        const txSig = await program.methods
          .stake(TEST_AMOUNT, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const event = await extractAnchorEvent(
          txSig,
          "StakeEvent",
          parseStakeEventData
        );
        expect(event).to.not.be.null;
        expect(event!.sourceContract).to.equal(
          program.programId.toBase58()
        );
        expect(event!.amount).to.equal(BigInt(TEST_AMOUNT.toString()));
        expect(event!.receiverAddress).to.equal(receiverAddress);
        expect(event!.nonce).to.equal(BigInt(nonceBefore + 1));
        expect(event!.chainId).to.equal(
          BigInt(SOURCE_CHAIN_ID.toString())
        );
      });
    });
  });

  // ===================================================================
  // Receiver Contract Tests (mirror SVM TC-101~TC-111)
  // ===================================================================

  describe("Receiver Contract Tests", () => {
    describe("TC-101: Add relayers", () => {
      it("should add relayers with admin", async () => {
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

        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.relayerCount.toNumber()).to.equal(3);
      });
    });

    describe("TC-102: Remove relayer", () => {
      it("should remove relayer and decrement count", async () => {
        await program.methods
          .removeRelayer(relayer1.publicKey)
          .accounts({
            admin: admin.publicKey,
            receiverState: receiverState,
            systemProgram: SystemProgram.programId,
          })
          .signers([admin])
          .rpc();

        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.relayerCount.toNumber()).to.equal(2);
      });
    });

    describe("TC-103: Non-admin relayer management", () => {
      it("should reject non-admin add relayer", async () => {
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
            .removeRelayer(relayer2.publicKey)
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

    describe("TC-104: Submit signature - single relayer (below threshold)", () => {
      it("should accept signature but not unlock when threshold not reached", async () => {
        // Re-add relayer1 (was removed in TC-102)
        await program.methods
          .addRelayer(relayer1.publicKey)
          .accounts({
            admin: admin.publicKey,
            receiverState: receiverState,
            systemProgram: SystemProgram.programId,
          })
          .signers([admin])
          .rpc();

        const eventData: SubmitEventData = {
          nonce: new BN(1),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1000),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);

        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toNumber()).to.equal(0);
      });
    });

    describe("TC-105: Submit signature - threshold reached, unlock", () => {
      it("should unlock when threshold is reached and emit CrossChainSuccessEvent", async () => {
        const eventData: SubmitEventData = {
          nonce: new BN(2),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1001),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        const user2TokenAccount = await getAssociatedTokenAddress(
          usdcMint,
          user2.publicKey
        );
        const vaultBefore = await getAccount(
          provider.connection,
          vaultTokenAccount
        );
        const receiverBefore = await getAccount(
          provider.connection,
          user2TokenAccount
        );

        await submitSignatureWithEd25519(relayer1, eventData, eventData.nonce);
        const unlockTxSig = await submitSignatureWithEd25519(
          relayer2,
          eventData,
          eventData.nonce
        );

        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toNumber()).to.equal(2);

        const vaultAfter = await getAccount(
          provider.connection,
          vaultTokenAccount
        );
        const receiverAfter = await getAccount(
          provider.connection,
          user2TokenAccount
        );
        expect(
          Number(vaultBefore.amount - vaultAfter.amount)
        ).to.equal(TEST_AMOUNT.toNumber());
        expect(
          Number(receiverAfter.amount - receiverBefore.amount)
        ).to.equal(TEST_AMOUNT.toNumber());

        const successEvent = await extractAnchorEvent(
          unlockTxSig,
          "CrossChainSuccessEvent",
          parseCrossChainSuccessEventData
        );
        expect(successEvent).to.not.be.null;
        expect(successEvent!.amount).to.equal(
          BigInt(TEST_AMOUNT.toString())
        );
        expect(successEvent!.nonce).to.equal(BigInt(2));
        expect(successEvent!.receiverAddress).to.equal(
          user2.publicKey.toBase58()
        );
      });
    });

    describe("TC-106: Nonce replay protection", () => {
      it("should accept larger nonce (normal case)", async () => {
        const eventData: SubmitEventData = {
          nonce: new BN(3),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1004),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        await submitSignatureWithEd25519(
          relayer1,
          eventData,
          eventData.nonce
        );
      });
    });

    describe("TC-108: Non-whitelisted relayer rejected", () => {
      it("should reject non-whitelisted relayer", async () => {
        const eventData: SubmitEventData = {
          nonce: new BN(5),
          amount: TEST_AMOUNT,
          blockHeight: new BN(1006),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        try {
          await submitSignatureWithEd25519(
            nonRelayer,
            eventData,
            eventData.nonce
          );
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });
  });

  // ===================================================================
  // Integration Tests (mirror SVM IT-001~IT-004)
  // ===================================================================

  describe("Integration Tests", () => {
    describe("IT-001: End-to-end cross-chain transfer", () => {
      it("should complete end-to-end cross-chain transfer", async () => {
        const receiverAddress = user2.publicKey.toBase58();

        const userBefore = await getAccount(
          provider.connection,
          user1TokenAccount
        );

        const accounts = await getStakeAccounts(user1);
        await program.methods
          .stake(TEST_AMOUNT, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const userAfterStake = await getAccount(
          provider.connection,
          user1TokenAccount
        );
        expect(
          Number(userBefore.amount - userAfterStake.amount)
        ).to.equal(TEST_AMOUNT.toNumber());

        const receiverStateAccountBefore =
          await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const validNonce = lastNonce.add(new BN(10));

        const eventData: SubmitEventData = {
          nonce: validNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(2000),
          sender: new Array(32).fill(0),
          receiverAddress: new PublicKey(receiverAddress),
        };

        const user2TokenAccount = await getAssociatedTokenAddress(
          usdcMint,
          user2.publicKey
        );
        const vaultBefore = await getAccount(
          provider.connection,
          vaultTokenAccount
        );
        const receiverBefore = await getAccount(
          provider.connection,
          user2TokenAccount
        );

        await submitSignatureWithEd25519(
          relayer2,
          eventData,
          eventData.nonce
        );
        await submitSignatureWithEd25519(
          relayer3,
          eventData,
          eventData.nonce
        );

        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toString()).to.equal(
          validNonce.toString()
        );

        const vaultAfter = await getAccount(
          provider.connection,
          vaultTokenAccount
        );
        const receiverAfter = await getAccount(
          provider.connection,
          user2TokenAccount
        );
        expect(
          Number(vaultBefore.amount - vaultAfter.amount)
        ).to.equal(TEST_AMOUNT.toNumber());
        expect(
          Number(receiverAfter.amount - receiverBefore.amount)
        ).to.equal(TEST_AMOUNT.toNumber());
      });
    });

    describe("IT-003: Sequential stakes", () => {
      it("should handle sequential stakes correctly", async () => {
        const stateBefore =
          await program.account.senderState.fetch(senderState);
        const nonceBefore = stateBefore.nonce;

        let successCount = 0;
        const accounts = await getStakeAccounts(user1);
        for (let i = 0; i < 10; i++) {
          try {
            await program.methods
              .stake(
                new BN(1_000_000),
                user2.publicKey.toBase58()
              )
              .accounts(accounts)
              .signers([user1])
              .rpc();
            successCount++;
          } catch (_) {}
        }

        const stateAfter =
          await program.account.senderState.fetch(senderState);
        const successfulStakes = stateAfter.nonce
          .sub(nonceBefore)
          .toNumber();

        expect(successfulStakes).to.be.greaterThanOrEqual(8);
        expect(successCount).to.be.greaterThanOrEqual(8);
      });
    });

    describe("IT-004: Large amount transfer", () => {
      it("should handle large amount transfer", async () => {
        const largeAmount = new BN(10_000_000_000);
        const receiverAddress = user2.publicKey.toBase58();

        const accounts = await getStakeAccounts(user1);
        await program.methods
          .stake(largeAmount, receiverAddress)
          .accounts(accounts)
          .signers([user1])
          .rpc();

        const senderStateAccount =
          await program.account.senderState.fetch(senderState);
        expect(senderStateAccount.nonce.toNumber()).to.be.greaterThan(0);
      });
    });
  });

  // ===================================================================
  // Security Tests (mirror SVM ST-001~ST-005)
  // ===================================================================

  describe("Security Tests", () => {
    describe("ST-001: Nonce replay defense", () => {
      it("should reject same nonce replay attack", async () => {
        const receiverStateAccountBefore =
          await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const testNonce = lastNonce.add(new BN(1));

        const eventData: SubmitEventData = {
          nonce: testNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(3000),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        await submitSignatureWithEd25519(
          relayer1,
          eventData,
          eventData.nonce
        );
        await submitSignatureWithEd25519(
          relayer2,
          eventData,
          eventData.nonce
        );

        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toString()).to.equal(
          testNonce.toString()
        );

        try {
          await submitSignatureWithEd25519(
            relayer1,
            eventData,
            eventData.nonce
          );
          expect.fail("Should have thrown an error");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });

    describe("ST-002: RelayerAlreadySigned", () => {
      it("should reject duplicate signature from same relayer on same nonce", async () => {
        const receiverStateAccountBefore =
          await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const testNonce = lastNonce.add(new BN(50));

        const eventData: SubmitEventData = {
          nonce: testNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(3100),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        await submitSignatureWithEd25519(
          relayer1,
          eventData,
          eventData.nonce
        );

        const [crossChainRequestPda] = getCrossChainRequestPDA(testNonce);
        const requestBefore =
          await program.account.crossChainRequest.fetch(crossChainRequestPda);
        expect(requestBefore.signatureCount).to.equal(1);

        try {
          await submitSignatureWithEd25519(
            relayer1,
            eventData,
            eventData.nonce
          );
          expect.fail("Should have thrown RelayerAlreadySigned");
        } catch (err: any) {
          expect(err).to.exist;
        }
      });
    });

    describe("ST-003: Permission control", () => {
      it("should reject non-admin add relayer", async () => {
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

    describe("ST-004: Event data mismatch rejection", () => {
      it("should reject second relayer submitting different event data for same nonce", async () => {
        const receiverStateAccountBefore =
          await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const testNonce = lastNonce.add(new BN(100));

        const eventData1: SubmitEventData = {
          nonce: testNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(4000),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        await submitSignatureWithEd25519(
          relayer1,
          eventData1,
          eventData1.nonce
        );

        const eventData2: SubmitEventData = {
          nonce: testNonce,
          amount: new BN(999_000000),
          blockHeight: new BN(4000),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        try {
          await submitSignatureWithEd25519(
            relayer2,
            eventData2,
            eventData2.nonce
          );
          expect.fail("Should have thrown InvalidEventData");
        } catch (err: any) {
          expect(err).to.exist;
        }
      });
    });

    describe("ST-005: PDA nonce isolation", () => {
      it("should store independent state for different nonces", async () => {
        const receiverStateAccountBefore =
          await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const nonce1 = lastNonce.add(new BN(200));
        const nonce2 = lastNonce.add(new BN(201));

        const eventData1: SubmitEventData = {
          nonce: nonce1,
          amount: TEST_AMOUNT,
          blockHeight: new BN(5002),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        const eventData2: SubmitEventData = {
          nonce: nonce2,
          amount: new BN(50_000000),
          blockHeight: new BN(5003),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        await submitSignatureWithEd25519(
          relayer1,
          eventData1,
          eventData1.nonce
        );
        await submitSignatureWithEd25519(
          relayer1,
          eventData2,
          eventData2.nonce
        );

        const [pda1] = getCrossChainRequestPDA(nonce1);
        const [pda2] = getCrossChainRequestPDA(nonce2);

        expect(pda1.toBase58()).to.not.equal(pda2.toBase58());

        const request1 =
          await program.account.crossChainRequest.fetch(pda1);
        const request2 =
          await program.account.crossChainRequest.fetch(pda2);

        expect(request1.eventData.amount.toString()).to.equal(
          TEST_AMOUNT.toString()
        );
        expect(request2.eventData.amount.toString()).to.equal(
          new BN(50_000000).toString()
        );
        expect(request1.signatureCount).to.equal(1);
        expect(request2.signatureCount).to.equal(1);
        expect(request1.nonce.toString()).to.equal(nonce1.toString());
        expect(request2.nonce.toString()).to.equal(nonce2.toString());
      });
    });

    describe("ST-006: Unlock fails when receiver ATA does not exist", () => {
      it("should accept first signature but fail on threshold when receiver has no ATA", async () => {
        const userNoAta = Keypair.generate();

        const receiverStateAccountBefore =
          await program.account.receiverState.fetch(receiverState);
        const lastNonce = receiverStateAccountBefore.lastNonce;
        const testNonce = lastNonce.add(new BN(300));

        const eventData: SubmitEventData = {
          nonce: testNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(6000),
          sender: new Array(32).fill(0),
          receiverAddress: userNoAta.publicKey,
        };

        // First signature: below threshold, should succeed even though ATA doesn't exist
        await submitSignatureWithEd25519(
          relayer1,
          eventData,
          eventData.nonce
        );

        const [crossChainRequestPda] = getCrossChainRequestPDA(testNonce);
        const requestAfterFirst =
          await program.account.crossChainRequest.fetch(crossChainRequestPda);
        expect(requestAfterFirst.signatureCount).to.equal(1);
        expect(requestAfterFirst.isUnlocked).to.be.false;

        // Second signature: reaches threshold, triggers unlock transfer
        // Should fail because receiver's USDC ATA does not exist
        try {
          await submitSignatureWithEd25519(
            relayer2,
            eventData,
            eventData.nonce
          );
          expect.fail("Should have failed: receiver ATA does not exist");
        } catch (err: any) {
          expect(err).to.exist;
          // Verify the PDA was NOT marked as unlocked
          const requestAfterFail =
            await program.account.crossChainRequest.fetch(crossChainRequestPda);
          expect(requestAfterFail.isUnlocked).to.be.false;
          // signature_count should still be 1 because the tx was rolled back
          expect(requestAfterFail.signatureCount).to.equal(1);
        }
      });
    });

    describe("ST-007: Nonce overflow", () => {
      it("should handle u64::MAX nonce and block all subsequent nonces", async () => {
        const maxNonce = new BN("18446744073709551615");
        const eventData: SubmitEventData = {
          nonce: maxNonce,
          amount: TEST_AMOUNT,
          blockHeight: new BN(9000),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        await submitSignatureWithEd25519(
          relayer1,
          eventData,
          eventData.nonce
        );
        await submitSignatureWithEd25519(
          relayer2,
          eventData,
          eventData.nonce
        );

        const receiverStateAccount =
          await program.account.receiverState.fetch(receiverState);
        expect(receiverStateAccount.lastNonce.toString()).to.equal(
          maxNonce.toString()
        );

        const nextEventData: SubmitEventData = {
          nonce: new BN(1),
          amount: TEST_AMOUNT,
          blockHeight: new BN(9001),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        try {
          await submitSignatureWithEd25519(
            relayer1,
            nextEventData,
            nextEventData.nonce
          );
          expect.fail("Should reject: no nonce can exceed u64::MAX");
        } catch (err) {
          expect(err).to.exist;
        }
      });
    });
  });

  // ===================================================================
  // Liquidity Tests
  // ===================================================================

  describe("Liquidity Tests", () => {
    it("should add liquidity", async () => {
      const adminTokenAccount = await getAssociatedTokenAddress(
        usdcMint,
        admin.publicKey
      );

      try {
        await createAccount(
          provider.connection,
          admin,
          usdcMint,
          admin.publicKey
        );
      } catch (_) {}

      await mintTo(
        provider.connection,
        admin,
        usdcMint,
        adminTokenAccount,
        admin,
        1_000_000_000
      );

      const amount = new BN(50_000_000);
      const adminBefore = await getAccount(
        provider.connection,
        adminTokenAccount
      );
      const vaultBefore = await getAccount(
        provider.connection,
        vaultTokenAccount
      );

      await program.methods
        .addLiquidity(amount)
        .accounts({
          senderState,
          admin: admin.publicKey,
          vault,
          usdcMint,
          adminTokenAccount,
          vaultTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([admin])
        .rpc();

      const adminAfter = await getAccount(
        provider.connection,
        adminTokenAccount
      );
      const vaultAfter = await getAccount(
        provider.connection,
        vaultTokenAccount
      );

      expect(
        Number(adminBefore.amount - adminAfter.amount)
      ).to.equal(50_000_000);
      expect(
        Number(vaultAfter.amount - vaultBefore.amount)
      ).to.equal(50_000_000);
    });

    it("should withdraw liquidity", async () => {
      const adminTokenAccount = await getAssociatedTokenAddress(
        usdcMint,
        admin.publicKey
      );

      const amount = new BN(20_000_000);

      const adminBefore = await getAccount(
        provider.connection,
        adminTokenAccount
      );
      const vaultBefore = await getAccount(
        provider.connection,
        vaultTokenAccount
      );

      await program.methods
        .withdrawLiquidity(amount)
        .accounts({
          senderState,
          admin: admin.publicKey,
          vault,
          usdcMint,
          adminTokenAccount,
          vaultTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([admin])
        .rpc();

      const adminAfter = await getAccount(
        provider.connection,
        adminTokenAccount
      );
      const vaultAfter = await getAccount(
        provider.connection,
        vaultTokenAccount
      );

      expect(
        Number(adminAfter.amount - adminBefore.amount)
      ).to.equal(20_000_000);
      expect(
        Number(vaultBefore.amount - vaultAfter.amount)
      ).to.equal(20_000_000);
    });
  });

  // ===================================================================
  // Cryptographic Helper Tests (mirror SVM)
  // ===================================================================

  describe("Cryptographic Helper Tests", () => {
    describe("Threshold Calculation", () => {
      it("should calculate correct threshold for 3 relayers", () => {
        expect(calculateThreshold(3)).to.equal(2);
      });

      it("should calculate correct threshold for 4 relayers", () => {
        expect(calculateThreshold(4)).to.equal(3);
      });

      it("should calculate correct threshold for 5 relayers", () => {
        expect(calculateThreshold(5)).to.equal(4);
      });

      it("should calculate correct threshold for 18 relayers", () => {
        expect(calculateThreshold(18)).to.equal(12);
      });
    });

    describe("Ed25519 Serialization Consistency", () => {
      it("should produce consistent serialization for same event data", () => {
        const eventData: SubmitEventData = {
          nonce: new BN(40),
          amount: TEST_AMOUNT,
          blockHeight: new BN(8000),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        const buf1 = serializeEventData(eventData);
        const buf2 = serializeEventData(eventData);
        expect(buf1.toString("hex")).to.equal(buf2.toString("hex"));
      });
    });

    describe("Ed25519 Signature Generation and Verification", () => {
      it("should generate and verify valid signature", async () => {
        const eventData: SubmitEventData = {
          nonce: new BN(41),
          amount: TEST_AMOUNT,
          blockHeight: new BN(8001),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        const message = serializeEventData(eventData);
        const signature = await ed25519.sign(
          message,
          relayer1.secretKey.slice(0, 32)
        );
        const isValid = await ed25519.verify(
          signature,
          message,
          relayer1.publicKey.toBytes()
        );
        expect(isValid).to.be.true;
      });

      it("should reject wrong key verification", async () => {
        const eventData: SubmitEventData = {
          nonce: new BN(42),
          amount: TEST_AMOUNT,
          blockHeight: new BN(8002),
          sender: new Array(32).fill(0),
          receiverAddress: user2.publicKey,
        };

        const message = serializeEventData(eventData);
        const signature = await ed25519.sign(
          message,
          relayer1.secretKey.slice(0, 32)
        );
        const isValid = await ed25519.verify(
          signature,
          message,
          relayer2.publicKey.toBytes()
        );
        expect(isValid).to.be.false;
      });
    });
  });
});
