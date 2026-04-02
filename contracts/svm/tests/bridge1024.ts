import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Bridge1024 } from "../target/types/bridge1024";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_INSTRUCTIONS_PUBKEY,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert, expect } from "chai";

describe("bridge1024", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.Bridge1024 as Program<Bridge1024>;
  const connection = provider.connection;
  const admin = (provider.wallet as anchor.Wallet).payer;
  const adminPubkey = admin.publicKey;

  // ---- PDAs ----
  const [senderState] = PublicKey.findProgramAddressSync(
    [Buffer.from("sender_state")],
    program.programId,
  );
  const [receiverState] = PublicKey.findProgramAddressSync(
    [Buffer.from("receiver_state")],
    program.programId,
  );
  const [vault] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    program.programId,
  );

  // ---- Keypairs ----
  const user = Keypair.generate();
  const nonAdmin = Keypair.generate();
  const newAdmin = Keypair.generate();
  const relayer1 = Keypair.generate();
  const relayer2 = Keypair.generate();
  const relayer3 = Keypair.generate();

  // ---- Token state ----
  let usdcMint: PublicKey;
  let wrongMint: PublicKey;
  let vaultTokenAccount: PublicKey;
  let adminTokenAccount: PublicKey;
  let userTokenAccount: PublicKey;
  let userWrongMintAccount: PublicKey;

  const USDC_DECIMALS = 6;
  const INITIAL_USDC = 1_000_000_000; // 1 000 USDC

  // ---- Helpers ----

  function crossChainRequestPDA(nonce: anchor.BN): PublicKey {
    const [pda] = PublicKey.findProgramAddressSync(
      [Buffer.from("cross_chain_request"), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId,
    );
    return pda;
  }

  async function expectError(
    fn: () => Promise<any>,
    expectedError: string,
  ): Promise<void> {
    let threw = false;
    try {
      await fn();
    } catch (err: any) {
      threw = true;
      const errMsg =
        err.error?.errorCode?.code ||
        err.error?.errorMessage ||
        err.toString();
      assert.ok(
        errMsg.includes(expectedError),
        `Expected error containing "${expectedError}" but got: ${errMsg}`,
      );
    }
    if (!threw) {
      assert.fail("Expected transaction to fail but it succeeded");
    }
  }

  async function airdropSol(pubkey: PublicKey, sol: number): Promise<void> {
    const sig = await connection.requestAirdrop(
      pubkey,
      sol * LAMPORTS_PER_SOL,
    );
    await connection.confirmTransaction(sig);
  }

  // =========================================================================
  // Setup
  // =========================================================================

  before(async () => {
    // Fund test wallets
    await Promise.all([
      airdropSol(user.publicKey, 10),
      airdropSol(nonAdmin.publicKey, 10),
      airdropSol(newAdmin.publicKey, 10),
      airdropSol(relayer1.publicKey, 10),
    ]);

    // Create mock USDC mint (admin is mint authority)
    usdcMint = await createMint(
      connection,
      admin,
      adminPubkey,
      null,
      USDC_DECIMALS,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID,
    );

    // Create a second mint for negative tests
    wrongMint = await createMint(
      connection,
      admin,
      adminPubkey,
      null,
      USDC_DECIMALS,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID,
    );

    // Create token accounts
    const vaultTokenKeypair = Keypair.generate();
    vaultTokenAccount = await createAccount(
      connection,
      admin,
      usdcMint,
      vault,
      vaultTokenKeypair,
    );
    adminTokenAccount = await createAccount(
      connection,
      admin,
      usdcMint,
      adminPubkey,
    );
    userTokenAccount = await createAccount(
      connection,
      admin,
      usdcMint,
      user.publicKey,
    );
    userWrongMintAccount = await createAccount(
      connection,
      admin,
      wrongMint,
      user.publicKey,
    );

    // Mint USDC to user and admin
    await mintTo(
      connection,
      admin,
      usdcMint,
      userTokenAccount,
      admin,
      INITIAL_USDC,
    );
    await mintTo(
      connection,
      admin,
      usdcMint,
      adminTokenAccount,
      admin,
      INITIAL_USDC,
    );
    await mintTo(
      connection,
      admin,
      wrongMint,
      userWrongMintAccount,
      admin,
      INITIAL_USDC,
    );
  });

  // =========================================================================
  // 1. Initialization
  // =========================================================================

  describe("Initialization", () => {
    it("initializes bridge state", async () => {
      await program.methods
        .initialize()
        .accounts({
          senderState,
          receiverState,
          admin: adminPubkey,
          vault,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      const ss = await program.account.senderState.fetch(senderState);
      assert.ok(ss.admin.equals(adminPubkey), "Admin should be deployer");
      assert.ok(ss.vault.equals(vault), "Vault PDA should be stored");
      assert.equal(ss.nonce.toNumber(), 0, "Nonce starts at 0");
      assert.equal(ss.isPaused, false, "Not paused initially");
      assert.ok(
        ss.pendingAdmin.equals(PublicKey.default),
        "No pending admin initially",
      );

      const rs = await program.account.receiverState.fetch(receiverState);
      assert.ok(rs.admin.equals(adminPubkey), "Receiver admin is deployer");
      assert.ok(rs.vault.equals(vault), "Receiver vault matches");
      assert.equal(rs.relayerCount.toNumber(), 0, "No relayers initially");
      assert.equal(rs.bridgeFee.toNumber(), 0, "Bridge fee starts at 0");
      assert.equal(rs.isPaused, false, "Receiver not paused");
      assert.equal(rs.lastNonce.toNumber(), 0, "Last nonce starts at 0");
    });
  });

  // =========================================================================
  // 2. Configuration
  // =========================================================================

  describe("Configuration", () => {
    it("configure USDC mint", async () => {
      await program.methods
        .configureUsdc(usdcMint)
        .accounts({
          senderState,
          receiverState,
          admin: adminPubkey,
        })
        .rpc();

      const ss = await program.account.senderState.fetch(senderState);
      assert.ok(ss.usdcMint.equals(usdcMint), "Sender USDC mint set");

      const rs = await program.account.receiverState.fetch(receiverState);
      assert.ok(rs.usdcMint.equals(usdcMint), "Receiver USDC mint set");
    });

    it("configure peer — verifies chain ID swapping", async () => {
      const peerContract =
        "0x1234567890abcdef1234567890abcdef12345678";
      const sourceChainId = new anchor.BN(1);
      const targetChainId = new anchor.BN(1024);

      await program.methods
        .configurePeer(peerContract, sourceChainId, targetChainId)
        .accounts({
          senderState,
          receiverState,
          admin: adminPubkey,
        })
        .rpc();

      const ss = await program.account.senderState.fetch(senderState);
      assert.equal(ss.targetContract, peerContract);
      assert.equal(ss.sourceChainId.toNumber(), 1);
      assert.equal(ss.targetChainId.toNumber(), 1024);

      // Receiver gets swapped chain IDs
      const rs = await program.account.receiverState.fetch(receiverState);
      assert.equal(rs.sourceContract, peerContract);
      assert.equal(
        rs.sourceChainId.toNumber(),
        1024,
        "Receiver source = sender target",
      );
      assert.equal(
        rs.targetChainId.toNumber(),
        1,
        "Receiver target = sender source",
      );
    });

    it("configure fee", async () => {
      const fee = new anchor.BN(100_000); // 0.1 USDC
      await program.methods
        .configureFee(fee)
        .accounts({
          receiverState,
          admin: adminPubkey,
        })
        .rpc();

      const rs = await program.account.receiverState.fetch(receiverState);
      assert.equal(rs.bridgeFee.toNumber(), 100_000);
    });

    it("non-admin cannot configure", async () => {
      await expectError(
        () =>
          program.methods
            .configureUsdc(usdcMint)
            .accounts({
              senderState,
              receiverState,
              admin: nonAdmin.publicKey,
            })
            .signers([nonAdmin])
            .rpc(),
        "Unauthorized",
      );
    });
  });

  // =========================================================================
  // 3. Relayer Management
  // =========================================================================

  describe("Relayer Management", () => {
    it("add relayer", async () => {
      await program.methods
        .addRelayer(relayer1.publicKey)
        .accounts({
          receiverState,
          admin: adminPubkey,
        })
        .rpc();

      const rs = await program.account.receiverState.fetch(receiverState);
      assert.equal(rs.relayerCount.toNumber(), 1);
      assert.ok(
        rs.relayers.some((r: PublicKey) => r.equals(relayer1.publicKey)),
        "Relayer1 present",
      );
    });

    it("cannot add duplicate relayer", async () => {
      await expectError(
        () =>
          program.methods
            .addRelayer(relayer1.publicKey)
            .accounts({
              receiverState,
              admin: adminPubkey,
            })
            .rpc(),
        "RelayerAlreadyExists",
      );
    });

    it("rotate relayer", async () => {
      // Add relayer2 so there is something to rotate
      await program.methods
        .addRelayer(relayer2.publicKey)
        .accounts({
          receiverState,
          admin: adminPubkey,
        })
        .rpc();

      // Rotate relayer2 → relayer3
      await program.methods
        .rotateRelayer(relayer2.publicKey, relayer3.publicKey)
        .accounts({
          receiverState,
          admin: adminPubkey,
        })
        .rpc();

      const rs = await program.account.receiverState.fetch(receiverState);
      assert.ok(
        rs.relayers.some((r: PublicKey) => r.equals(relayer3.publicKey)),
        "Relayer3 present after rotation",
      );
      assert.ok(
        !rs.relayers.some((r: PublicKey) => r.equals(relayer2.publicKey)),
        "Relayer2 removed after rotation",
      );
      // Count unchanged by rotation
      assert.equal(rs.relayerCount.toNumber(), 2);
    });

    it("remove relayer", async () => {
      await program.methods
        .removeRelayer(relayer3.publicKey)
        .accounts({
          receiverState,
          admin: adminPubkey,
        })
        .rpc();

      const rs = await program.account.receiverState.fetch(receiverState);
      assert.equal(rs.relayerCount.toNumber(), 1);
      assert.ok(
        !rs.relayers.some((r: PublicKey) => r.equals(relayer3.publicKey)),
        "Relayer3 removed",
      );
    });

    it("cannot exceed max relayers", async () => {
      // Currently have relayer1 (count 1). Add 17 more to hit MAX_RELAYERS = 18.
      for (let i = 0; i < 17; i++) {
        await program.methods
          .addRelayer(Keypair.generate().publicKey)
          .accounts({
            receiverState,
            admin: adminPubkey,
          })
          .rpc();
      }

      const rs = await program.account.receiverState.fetch(receiverState);
      assert.equal(rs.relayerCount.toNumber(), 18);

      await expectError(
        () =>
          program.methods
            .addRelayer(Keypair.generate().publicKey)
            .accounts({
              receiverState,
              admin: adminPubkey,
            })
            .rpc(),
        "TooManyRelayers",
      );
    });

    it("non-admin cannot manage relayers", async () => {
      await expectError(
        () =>
          program.methods
            .addRelayer(Keypair.generate().publicKey)
            .accounts({
              receiverState,
              admin: nonAdmin.publicKey,
            })
            .signers([nonAdmin])
            .rpc(),
        "Unauthorized",
      );
    });
  });

  // =========================================================================
  // 4. Stake
  // =========================================================================

  describe("Stake", () => {
    it("stake USDC — verifies nonce increment", async () => {
      const ssBefore = await program.account.senderState.fetch(senderState);
      const nonceBefore = ssBefore.nonce.toNumber();

      const stakeAmount = new anchor.BN(10_000_000); // 10 USDC
      const receiverAddress = "0xReceiverAddress1234567890abcdef12345678";

      await program.methods
        .stake(stakeAmount, receiverAddress)
        .accounts({
          senderState,
          receiverState,
          user: user.publicKey,
          vault,
          usdcMint,
          userTokenAccount,
          vaultTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([user])
        .rpc();

      // Nonce incremented
      const ssAfter = await program.account.senderState.fetch(senderState);
      assert.equal(
        ssAfter.nonce.toNumber(),
        nonceBefore + 1,
        "Nonce should increment by 1",
      );

      // Vault received tokens
      const vaultAcct = await getAccount(connection, vaultTokenAccount);
      assert.ok(
        vaultAcct.amount >= BigInt(stakeAmount.toNumber()),
        "Vault should hold at least the staked amount",
      );
    });

    it("cannot stake when paused", async () => {
      // Pause
      await program.methods
        .pause()
        .accounts({ senderState, receiverState, admin: adminPubkey })
        .rpc();

      await expectError(
        () =>
          program.methods
            .stake(new anchor.BN(1_000_000), "0xReceiver")
            .accounts({
              senderState,
              receiverState,
              user: user.publicKey,
              vault,
              usdcMint,
              userTokenAccount,
              vaultTokenAccount,
              tokenProgram: TOKEN_PROGRAM_ID,
              systemProgram: SystemProgram.programId,
            })
            .signers([user])
            .rpc(),
        "Paused",
      );

      // Unpause for subsequent tests
      await program.methods
        .unpause()
        .accounts({ senderState, receiverState, admin: adminPubkey })
        .rpc();
    });

    it("cannot stake with wrong mint", async () => {
      // Create a vault-owned token account for the wrong mint so all token
      // account constraints are internally consistent.  The program rejects
      // because wrongMint != sender_state.usdc_mint.
      const wrongVaultKeypair = Keypair.generate();
      const wrongVaultTokenAccount = await createAccount(
        connection,
        admin,
        wrongMint,
        vault,
        wrongVaultKeypair,
      );

      await expectError(
        () =>
          program.methods
            .stake(new anchor.BN(1_000_000), "0xReceiver")
            .accounts({
              senderState,
              receiverState,
              user: user.publicKey,
              vault,
              usdcMint: wrongMint,
              userTokenAccount: userWrongMintAccount,
              vaultTokenAccount: wrongVaultTokenAccount,
              tokenProgram: TOKEN_PROGRAM_ID,
              systemProgram: SystemProgram.programId,
            })
            .signers([user])
            .rpc(),
        "UsdcNotConfigured",
      );
    });
  });

  // =========================================================================
  // 5. Submit Signature
  // =========================================================================

  describe("Submit Signature", () => {
    /*
     * TODO: Full Ed25519 signature verification end-to-end tests
     *
     * Testing the happy-path of submit_signature requires prepending a native
     * Ed25519Program instruction to the same transaction.  The on-chain
     * verifier enforces Wormhole-style rules:
     *
     *   • Exactly one signature per Ed25519 instruction
     *   • All three instruction-index fields (sig, pubkey, message) must be
     *     0xFFFF — i.e. data is embedded directly in the Ed25519 ix, not
     *     referenced from another instruction
     *
     * Building such an instruction in TypeScript requires:
     *
     *   1. Borsh-serializing StakeEventData (nonce u64, amount u64,
     *      block_height u64, sender [u8;32], receiver_address Pubkey) in
     *      the exact layout Anchor's AnchorSerialize produces.
     *   2. Signing the serialized bytes with the relayer's Ed25519 keypair.
     *   3. Packing the raw instruction buffer:
     *        [num_sigs=1, padding, sig_offset, sig_ix=0xFFFF,
     *         pk_offset, pk_ix=0xFFFF, msg_offset, msg_size,
     *         msg_ix=0xFFFF, signature(64), pubkey(32), message(N)]
     *   4. Adding the Ed25519 instruction at index 0 in the transaction
     *      so it precedes the submit_signature instruction.
     *
     * This level of raw instruction construction is fragile in JS tests and
     * is better covered by a dedicated integration test harness (e.g.
     * bankrun / solana-program-test with Rust-side helpers) or by using
     * @solana/web3.js Ed25519Program.createInstructionWithPrivateKey()
     * combined with manual transaction building.
     */

    it("non-relayer cannot submit signature", async () => {
      const nonce = new anchor.BN(1);
      const crossChainRequest = crossChainRequestPDA(nonce);

      const receiverTokenAccount = await createAccount(
        connection,
        admin,
        usdcMint,
        nonAdmin.publicKey,
      );

      const eventData = {
        nonce,
        amount: new anchor.BN(1_000_000),
        blockHeight: new anchor.BN(100),
        sender: Array.from(new Uint8Array(32)),
        receiverAddress: nonAdmin.publicKey,
      };

      await expectError(
        () =>
          program.methods
            .submitSignature(nonce, eventData, Buffer.alloc(64))
            .accounts({
              receiverState,
              crossChainRequest,
              relayer: nonAdmin.publicKey,
              vault,
              usdcMint,
              vaultTokenAccount,
              receiverTokenAccount,
              instructionsSysvar: SYSVAR_INSTRUCTIONS_PUBKEY,
              tokenProgram: TOKEN_PROGRAM_ID,
              systemProgram: SystemProgram.programId,
            })
            .signers([nonAdmin])
            .rpc(),
        "RelayerNotFound",
      );
    });

    it("paused state blocks signature submission", async () => {
      await program.methods
        .pause()
        .accounts({ senderState, receiverState, admin: adminPubkey })
        .rpc();

      const nonce = new anchor.BN(2);
      const crossChainRequest = crossChainRequestPDA(nonce);

      const receiverTokenAccount = await createAccount(
        connection,
        admin,
        usdcMint,
        relayer1.publicKey,
      );

      const eventData = {
        nonce,
        amount: new anchor.BN(1_000_000),
        blockHeight: new anchor.BN(100),
        sender: Array.from(new Uint8Array(32)),
        receiverAddress: relayer1.publicKey,
      };

      await expectError(
        () =>
          program.methods
            .submitSignature(nonce, eventData, Buffer.alloc(64))
            .accounts({
              receiverState,
              crossChainRequest,
              relayer: relayer1.publicKey,
              vault,
              usdcMint,
              vaultTokenAccount,
              receiverTokenAccount,
              instructionsSysvar: SYSVAR_INSTRUCTIONS_PUBKEY,
              tokenProgram: TOKEN_PROGRAM_ID,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc(),
        "Paused",
      );

      // Unpause for subsequent tests
      await program.methods
        .unpause()
        .accounts({ senderState, receiverState, admin: adminPubkey })
        .rpc();
    });
  });

  // =========================================================================
  // 6. Admin Transfer (2-step)
  // =========================================================================

  describe("Admin Transfer", () => {
    it("propose and accept admin transfer", async () => {
      // Step 1 — propose
      await program.methods
        .proposeAdmin(newAdmin.publicKey)
        .accounts({ senderState, receiverState, admin: adminPubkey })
        .rpc();

      let ss = await program.account.senderState.fetch(senderState);
      assert.ok(ss.pendingAdmin.equals(newAdmin.publicKey));

      let rs = await program.account.receiverState.fetch(receiverState);
      assert.ok(rs.pendingAdmin.equals(newAdmin.publicKey));

      // Step 2 — accept
      await program.methods
        .acceptAdmin()
        .accounts({
          senderState,
          receiverState,
          newAdmin: newAdmin.publicKey,
        })
        .signers([newAdmin])
        .rpc();

      ss = await program.account.senderState.fetch(senderState);
      assert.ok(ss.admin.equals(newAdmin.publicKey), "Admin updated");
      assert.ok(
        ss.pendingAdmin.equals(PublicKey.default),
        "Pending admin cleared",
      );

      rs = await program.account.receiverState.fetch(receiverState);
      assert.ok(rs.admin.equals(newAdmin.publicKey));

      // Restore original admin so remaining tests keep working
      await program.methods
        .proposeAdmin(adminPubkey)
        .accounts({
          senderState,
          receiverState,
          admin: newAdmin.publicKey,
        })
        .signers([newAdmin])
        .rpc();

      await program.methods
        .acceptAdmin()
        .accounts({
          senderState,
          receiverState,
          newAdmin: adminPubkey,
        })
        .rpc();

      ss = await program.account.senderState.fetch(senderState);
      assert.ok(ss.admin.equals(adminPubkey), "Admin restored");
    });

    it("non-pending account cannot accept admin", async () => {
      // Propose newAdmin
      await program.methods
        .proposeAdmin(newAdmin.publicKey)
        .accounts({ senderState, receiverState, admin: adminPubkey })
        .rpc();

      // nonAdmin tries to accept — should fail
      await expectError(
        () =>
          program.methods
            .acceptAdmin()
            .accounts({
              senderState,
              receiverState,
              newAdmin: nonAdmin.publicKey,
            })
            .signers([nonAdmin])
            .rpc(),
        "Unauthorized",
      );

      // Clean up: accept with the real pending admin and transfer back
      await program.methods
        .acceptAdmin()
        .accounts({
          senderState,
          receiverState,
          newAdmin: newAdmin.publicKey,
        })
        .signers([newAdmin])
        .rpc();

      await program.methods
        .proposeAdmin(adminPubkey)
        .accounts({
          senderState,
          receiverState,
          admin: newAdmin.publicKey,
        })
        .signers([newAdmin])
        .rpc();

      await program.methods
        .acceptAdmin()
        .accounts({
          senderState,
          receiverState,
          newAdmin: adminPubkey,
        })
        .rpc();
    });
  });

  // =========================================================================
  // 7. Pause / Unpause
  // =========================================================================

  describe("Pause / Unpause", () => {
    it("admin can pause and unpause", async () => {
      await program.methods
        .pause()
        .accounts({ senderState, receiverState, admin: adminPubkey })
        .rpc();

      let ss = await program.account.senderState.fetch(senderState);
      let rs = await program.account.receiverState.fetch(receiverState);
      assert.equal(ss.isPaused, true, "Sender paused");
      assert.equal(rs.isPaused, true, "Receiver paused");

      await program.methods
        .unpause()
        .accounts({ senderState, receiverState, admin: adminPubkey })
        .rpc();

      ss = await program.account.senderState.fetch(senderState);
      rs = await program.account.receiverState.fetch(receiverState);
      assert.equal(ss.isPaused, false, "Sender unpaused");
      assert.equal(rs.isPaused, false, "Receiver unpaused");
    });

    it("non-admin cannot pause", async () => {
      await expectError(
        () =>
          program.methods
            .pause()
            .accounts({
              senderState,
              receiverState,
              admin: nonAdmin.publicKey,
            })
            .signers([nonAdmin])
            .rpc(),
        "Unauthorized",
      );
    });
  });

  // =========================================================================
  // 8. Close Request
  // =========================================================================

  describe("Close Request", () => {
    /*
     * close_request requires a CrossChainRequest PDA that was created (and
     * fully unlocked) via submit_signature.  Without the Ed25519 harness we
     * cannot create one in these unit tests, so we verify the guard-rail
     * that prevents closing a non-existent (uninitialized) request PDA.
     *
     * The program enforces:
     *   require!(ctx.accounts.cross_chain_request.is_unlocked, ErrorCode::InvalidNonce)
     * so attempting to close a request that was never unlocked would return
     * InvalidNonce.  However, here the PDA does not exist at all, so Anchor
     * rejects at the account-deserialization layer before the instruction
     * body runs.
     */

    it("cannot close non-existent request", async () => {
      const fakeNonce = new anchor.BN(999);
      const crossChainRequest = crossChainRequestPDA(fakeNonce);

      await expectError(
        () =>
          program.methods
            .closeRequest(fakeNonce)
            .accounts({
              crossChainRequest,
              receiverState,
              admin: adminPubkey,
            })
            .rpc(),
        "AccountNotInitialized",
      );
    });
  });

  // =========================================================================
  // 9. Liquidity Management
  // =========================================================================

  describe("Liquidity Management", () => {
    it("add and withdraw liquidity", async () => {
      const vaultBefore = await getAccount(connection, vaultTokenAccount);
      const balanceBefore = vaultBefore.amount;

      const addAmount = new anchor.BN(50_000_000); // 50 USDC

      await program.methods
        .addLiquidity(addAmount)
        .accounts({
          receiverState,
          admin: adminPubkey,
          vault,
          usdcMint,
          vaultTokenAccount,
          adminTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .rpc();

      const vaultAfterAdd = await getAccount(connection, vaultTokenAccount);
      assert.equal(
        (vaultAfterAdd.amount - balanceBefore).toString(),
        addAmount.toString(),
        "Vault increased by deposit amount",
      );

      // Withdraw half of what we just added
      const withdrawAmount = new anchor.BN(25_000_000); // 25 USDC

      await program.methods
        .withdrawLiquidity(withdrawAmount)
        .accounts({
          receiverState,
          admin: adminPubkey,
          vault,
          usdcMint,
          vaultTokenAccount,
          adminTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .rpc();

      const vaultAfterWithdraw = await getAccount(
        connection,
        vaultTokenAccount,
      );
      assert.equal(
        (vaultAfterAdd.amount - vaultAfterWithdraw.amount).toString(),
        withdrawAmount.toString(),
        "Vault decreased by withdrawn amount",
      );

      const adminAcct = await getAccount(connection, adminTokenAccount);
      assert.ok(adminAcct.amount > 0n, "Admin received withdrawn tokens");
    });
  });
});
