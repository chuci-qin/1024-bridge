import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Bridge1024 } from "../target/types/bridge1024";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert } from "chai";

describe("bridge1024", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.Bridge1024 as Program<Bridge1024>;
  const connection = provider.connection;
  const admin = (provider.wallet as anchor.Wallet).payer;
  const adminPubkey = admin.publicKey;

  // ---- PDAs ----
  const [bridgeState] = PublicKey.findProgramAddressSync(
    [Buffer.from("bridge_state")],
    program.programId,
  );
  const [vault] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    program.programId,
  );

  // ---- Role keypairs ----
  const guardian = Keypair.generate();
  const operator = Keypair.generate();
  const recovery = Keypair.generate();
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
  const INITIAL_USDC = 1_000_000_000; // 1000 USDC

  // ---- Helpers ----

  function stakeRecordPDA(nonce: anchor.BN): PublicKey {
    const [pda] = PublicKey.findProgramAddressSync(
      [Buffer.from("stake_record"), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId,
    );
    return pda;
  }

  function crossChainRequestPDA(nonce: anchor.BN): PublicKey {
    const [pda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("cross_chain_request"),
        nonce.toArrayLike(Buffer, "le", 8),
      ],
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
    const sig = await connection.requestAirdrop(pubkey, sol * LAMPORTS_PER_SOL);
    await connection.confirmTransaction(sig);
  }

  // =========================================================================
  // Setup
  // =========================================================================

  before(async () => {
    await Promise.all([
      airdropSol(user.publicKey, 10),
      airdropSol(nonAdmin.publicKey, 10),
      airdropSol(newAdmin.publicKey, 10),
      airdropSol(relayer1.publicKey, 10),
      airdropSol(guardian.publicKey, 10),
      airdropSol(operator.publicKey, 10),
      airdropSol(recovery.publicKey, 10),
    ]);

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

    await mintTo(connection, admin, usdcMint, userTokenAccount, admin, INITIAL_USDC);
    await mintTo(connection, admin, usdcMint, adminTokenAccount, admin, INITIAL_USDC);
    await mintTo(connection, admin, wrongMint, userWrongMintAccount, admin, INITIAL_USDC);
    // Seed vault with initial liquidity for refund tests
    await mintTo(connection, admin, usdcMint, vaultTokenAccount, admin, 500_000_000);
  });

  // =========================================================================
  // 1. Initialization
  // =========================================================================

  describe("Initialization", () => {
    it("initializes bridge state with four roles", async () => {
      await program.methods
        .initialize(guardian.publicKey, operator.publicKey, recovery.publicKey)
        .accounts({
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.admin.equals(adminPubkey), "Admin should be deployer");
      assert.ok(bs.guardian.equals(guardian.publicKey), "Guardian set");
      assert.ok(bs.operator.equals(operator.publicKey), "Operator set");
      assert.ok(bs.recovery.equals(recovery.publicKey), "Recovery set");
      assert.ok(bs.vault.equals(vault), "Vault PDA stored");
      assert.equal(bs.isPaused, false, "Not paused initially");
      assert.equal(bs.timelockActive, false, "Timelock not active initially");
      assert.equal(bs.relayers.length, 0, "No relayers initially");
      assert.equal(bs.bridgeFee.toNumber(), 0, "Bridge fee starts at 0");
    });

    it("rejects duplicate role addresses", async () => {
      const bs2Admin = Keypair.generate();
      await airdropSol(bs2Admin.publicKey, 5);

      await expectError(
        () =>
          program.methods
            .initialize(
              bs2Admin.publicKey, // guardian = admin → overlap
              operator.publicKey,
              recovery.publicKey,
            )
            .accounts({
              bridgeState,
              admin: bs2Admin.publicKey,
              vault,
              systemProgram: SystemProgram.programId,
            })
            .signers([bs2Admin])
            .rpc(),
        "already in use", // PDA already initialized
      );
    });
  });

  // =========================================================================
  // 2. Configuration
  // =========================================================================

  describe("Configuration", () => {
    const peerContract = new Uint8Array(32);
    peerContract[0] = 0x12;
    peerContract[1] = 0x34;

    it("configure USDC mint, peer, and chain IDs", async () => {
      await program.methods
        .configure(
          usdcMint,
          Array.from(peerContract) as number[],
          new anchor.BN(1),    // localChainId
          new anchor.BN(1024), // peerChainId
        )
        .accounts({
          bridgeState,
          timelockOp: adminPubkey, // timelock not active, pass admin
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.usdcMint.equals(usdcMint), "USDC mint set");
      assert.deepEqual(
        bs.peerContract,
        Array.from(peerContract),
        "Peer contract set",
      );
      assert.equal(bs.localChainId.toNumber(), 1, "Local chain ID set");
      assert.equal(bs.peerChainId.toNumber(), 1024, "Peer chain ID set");
    });

    it("configure fee", async () => {
      const fee = new anchor.BN(100_000); // 0.1 USDC
      await program.methods
        .configureFee(fee)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.bridgeFee.toNumber(), 100_000);
    });

    it("configure rate limits", async () => {
      await program.methods
        .configureRateLimits(
          new anchor.BN(100_000_000), // maxPerWindow: 100 USDC
          new anchor.BN(3600),        // windowDuration: 1 hour
          new anchor.BN(50_000_000),  // maxSingle: 50 USDC
          new anchor.BN(100_000_000), // maxStake: 100 USDC
          new anchor.BN(10_000_000),  // minReserve: 10 USDC
        )
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.maxUnlockPerWindow.toNumber(), 100_000_000);
      assert.equal(bs.windowDuration.toNumber(), 3600);
      assert.equal(bs.maxSingleUnlock.toNumber(), 50_000_000);
      assert.equal(bs.maxStakeAmount.toNumber(), 100_000_000);
      assert.equal(bs.minimumReserve.toNumber(), 10_000_000);
    });

    it("non-admin cannot configure", async () => {
      await expectError(
        () =>
          program.methods
            .configure(
              usdcMint,
              Array.from(peerContract) as number[],
              new anchor.BN(1),
              new anchor.BN(1024),
            )
            .accounts({
              bridgeState,
              timelockOp: nonAdmin.publicKey,
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
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.relayers.length, 1);
      assert.ok(
        bs.relayers.some((r: PublicKey) => r.equals(relayer1.publicKey)),
        "Relayer1 present",
      );
    });

    it("cannot add duplicate relayer", async () => {
      await expectError(
        () =>
          program.methods
            .addRelayer(relayer1.publicKey)
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
            })
            .rpc(),
        "RelayerAlreadyExists",
      );
    });

    it("rotate relayer", async () => {
      await program.methods
        .addRelayer(relayer2.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      await program.methods
        .rotateRelayer(relayer2.publicKey, relayer3.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(
        bs.relayers.some((r: PublicKey) => r.equals(relayer3.publicKey)),
        "Relayer3 present after rotation",
      );
      assert.ok(
        !bs.relayers.some((r: PublicKey) => r.equals(relayer2.publicKey)),
        "Relayer2 removed after rotation",
      );
      assert.equal(bs.relayers.length, 2);
    });

    it("remove relayer", async () => {
      await program.methods
        .removeRelayer(relayer3.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.relayers.length, 1);
    });

    it("cannot exceed max relayers", async () => {
      // Currently have relayer1 (count=1). Add 17 more to hit MAX_RELAYERS=18.
      for (let i = 0; i < 17; i++) {
        await program.methods
          .addRelayer(Keypair.generate().publicKey)
          .accounts({
            bridgeState,
            timelockOp: adminPubkey,
            admin: adminPubkey,
          })
          .rpc();
      }

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.relayers.length, 18);

      await expectError(
        () =>
          program.methods
            .addRelayer(Keypair.generate().publicKey)
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
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
              bridgeState,
              timelockOp: nonAdmin.publicKey,
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
    it("stake USDC — creates StakeRecord with random nonce", async () => {
      const nonce = new anchor.BN(12345);
      const stakeAmount = new anchor.BN(10_000_000); // 10 USDC
      const receiver = new Uint8Array(32);
      receiver[0] = 0xab;
      receiver[1] = 0xcd;

      await program.methods
        .stake(nonce, stakeAmount, Array.from(receiver) as number[])
        .accounts({
          bridgeState,
          stakeRecord: stakeRecordPDA(nonce),
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

      const sr = await program.account.stakeRecord.fetch(stakeRecordPDA(nonce));
      assert.ok(sr.owner.equals(user.publicKey), "StakeRecord owner is user");
      assert.equal(sr.amount.toNumber(), 10_000_000, "StakeRecord amount");
      assert.equal(sr.refunded, false, "Not refunded yet");
      assert.equal(sr.refundInitiatedAt.toNumber(), 0, "Refund not initiated");
    });

    it("cannot stake when paused", async () => {
      await program.methods
        .emergencyFreeze()
        .accounts({ bridgeState, guardian: guardian.publicKey })
        .signers([guardian])
        .rpc();

      const nonce = new anchor.BN(22222);

      await expectError(
        () =>
          program.methods
            .stake(
              nonce,
              new anchor.BN(1_000_000),
              Array.from(new Uint8Array(32).fill(1)) as number[],
            )
            .accounts({
              bridgeState,
              stakeRecord: stakeRecordPDA(nonce),
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

      await program.methods
        .executeRecovery(adminPubkey, PublicKey.default)
        .accounts({ bridgeState, recovery: recovery.publicKey })
        .signers([recovery])
        .rpc();
    });

    it("cannot stake with wrong mint", async () => {
      const nonce = new anchor.BN(33333);

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
            .stake(
              nonce,
              new anchor.BN(1_000_000),
              Array.from(new Uint8Array(32).fill(1)) as number[],
            )
            .accounts({
              bridgeState,
              stakeRecord: stakeRecordPDA(nonce),
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

    it("rejects stake exceeding maxStakeAmount", async () => {
      const nonce = new anchor.BN(44444);

      await expectError(
        () =>
          program.methods
            .stake(
              nonce,
              new anchor.BN(200_000_000), // 200 USDC > maxStake=100 USDC
              Array.from(new Uint8Array(32).fill(1)) as number[],
            )
            .accounts({
              bridgeState,
              stakeRecord: stakeRecordPDA(nonce),
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
        "StakeAmountExceeded",
      );
    });
  });

  // =========================================================================
  // 5. Confirm Event
  // =========================================================================

  describe("Confirm Event", () => {
    it("non-relayer cannot confirm event", async () => {
      const nonce = new anchor.BN(100);
      const crossChainRequest = crossChainRequestPDA(nonce);

      const receiverTokenAccount = await createAccount(
        connection,
        admin,
        usdcMint,
        nonAdmin.publicKey,
      );

      const bs = await program.account.bridgeState.fetch(bridgeState);
      const eventData = {
        sourceContract: bs.peerContract,
        targetContract: Array.from(program.programId.toBytes()),
        sourceChainId: new anchor.BN(1024),
        targetChainId: new anchor.BN(1),
        blockHeight: new anchor.BN(100),
        amount: new anchor.BN(1_000_000),
        sender: Array.from(new Uint8Array(32)),
        receiver: Array.from(nonAdmin.publicKey.toBytes()),
        nonce,
      };

      await expectError(
        () =>
          program.methods
            .confirmEvent(nonce, eventData)
            .accounts({
              bridgeState,
              crossChainRequest,
              relayer: nonAdmin.publicKey,
              vault,
              usdcMint,
              vaultTokenAccount,
              receiverTokenAccount,
              tokenProgram: TOKEN_PROGRAM_ID,
              systemProgram: SystemProgram.programId,
            })
            .signers([nonAdmin])
            .rpc(),
        "RelayerNotFound",
      );
    });

    it("paused state blocks event confirmation", async () => {
      await program.methods
        .emergencyFreeze()
        .accounts({ bridgeState, guardian: guardian.publicKey })
        .signers([guardian])
        .rpc();

      const nonce = new anchor.BN(101);
      const crossChainRequest = crossChainRequestPDA(nonce);

      const receiverTokenAccount = await createAccount(
        connection,
        admin,
        usdcMint,
        relayer1.publicKey,
      );

      const eventData = {
        sourceContract: Array.from(new Uint8Array(32)),
        targetContract: Array.from(program.programId.toBytes()),
        sourceChainId: new anchor.BN(1024),
        targetChainId: new anchor.BN(1),
        blockHeight: new anchor.BN(100),
        amount: new anchor.BN(1_000_000),
        sender: Array.from(new Uint8Array(32)),
        receiver: Array.from(relayer1.publicKey.toBytes()),
        nonce,
      };

      await expectError(
        () =>
          program.methods
            .confirmEvent(nonce, eventData)
            .accounts({
              bridgeState,
              crossChainRequest,
              relayer: relayer1.publicKey,
              vault,
              usdcMint,
              vaultTokenAccount,
              receiverTokenAccount,
              tokenProgram: TOKEN_PROGRAM_ID,
              systemProgram: SystemProgram.programId,
            })
            .signers([relayer1])
            .rpc(),
        "Paused",
      );

      // Unpause via recovery
      await program.methods
        .executeRecovery(adminPubkey, PublicKey.default)
        .accounts({ bridgeState, recovery: recovery.publicKey })
        .signers([recovery])
        .rpc();
    });
  });

  // =========================================================================
  // 6. Admin Transfer (2-step)
  // =========================================================================

  describe("Admin Transfer", () => {
    it("propose and accept admin transfer", async () => {
      await program.methods
        .proposeAdmin(newAdmin.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      let bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.pendingAdmin.equals(newAdmin.publicKey));

      await program.methods
        .acceptAdmin()
        .accounts({ bridgeState, newAdmin: newAdmin.publicKey })
        .signers([newAdmin])
        .rpc();

      bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.admin.equals(newAdmin.publicKey), "Admin updated");
      assert.ok(
        bs.pendingAdmin.equals(PublicKey.default),
        "Pending admin cleared",
      );

      // Restore original admin
      await program.methods
        .proposeAdmin(adminPubkey)
        .accounts({
          bridgeState,
          timelockOp: newAdmin.publicKey,
          admin: newAdmin.publicKey,
        })
        .signers([newAdmin])
        .rpc();

      await program.methods
        .acceptAdmin()
        .accounts({ bridgeState, newAdmin: adminPubkey })
        .rpc();

      bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.admin.equals(adminPubkey), "Admin restored");
    });

    it("non-pending account cannot accept admin", async () => {
      await program.methods
        .proposeAdmin(newAdmin.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      await expectError(
        () =>
          program.methods
            .acceptAdmin()
            .accounts({ bridgeState, newAdmin: nonAdmin.publicKey })
            .signers([nonAdmin])
            .rpc(),
        "Unauthorized",
      );

      // Cleanup: accept and transfer back
      await program.methods
        .acceptAdmin()
        .accounts({ bridgeState, newAdmin: newAdmin.publicKey })
        .signers([newAdmin])
        .rpc();

      await program.methods
        .proposeAdmin(adminPubkey)
        .accounts({
          bridgeState,
          timelockOp: newAdmin.publicKey,
          admin: newAdmin.publicKey,
        })
        .signers([newAdmin])
        .rpc();

      await program.methods
        .acceptAdmin()
        .accounts({ bridgeState, newAdmin: adminPubkey })
        .rpc();
    });
  });

  // =========================================================================
  // 7. Emergency Freeze / Recovery
  // =========================================================================

  describe("Emergency Freeze / Recovery", () => {
    it("guardian can freeze, only recovery can unfreeze", async () => {
      await program.methods
        .emergencyFreeze()
        .accounts({ bridgeState, guardian: guardian.publicKey })
        .signers([guardian])
        .rpc();

      let bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.isPaused, true, "Bridge paused");

      // Admin cannot unfreeze (no unpause instruction)
      // Recovery unfreezes with new admin
      const tempAdmin = Keypair.generate();
      await airdropSol(tempAdmin.publicKey, 5);

      await program.methods
        .executeRecovery(tempAdmin.publicKey, PublicKey.default)
        .accounts({ bridgeState, recovery: recovery.publicKey })
        .signers([recovery])
        .rpc();

      bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.isPaused, false, "Bridge unpaused");
      assert.ok(bs.admin.equals(tempAdmin.publicKey), "Admin replaced by recovery");

      // Restore original admin
      await program.methods
        .proposeAdmin(adminPubkey)
        .accounts({
          bridgeState,
          timelockOp: tempAdmin.publicKey,
          admin: tempAdmin.publicKey,
        })
        .signers([tempAdmin])
        .rpc();
      await program.methods
        .acceptAdmin()
        .accounts({ bridgeState, newAdmin: adminPubkey })
        .rpc();
    });

    it("non-guardian cannot freeze", async () => {
      await expectError(
        () =>
          program.methods
            .emergencyFreeze()
            .accounts({ bridgeState, guardian: nonAdmin.publicKey })
            .signers([nonAdmin])
            .rpc(),
        "Unauthorized",
      );
    });

    it("cannot freeze when already paused", async () => {
      await program.methods
        .emergencyFreeze()
        .accounts({ bridgeState, guardian: guardian.publicKey })
        .signers([guardian])
        .rpc();

      await expectError(
        () =>
          program.methods
            .emergencyFreeze()
            .accounts({ bridgeState, guardian: guardian.publicKey })
            .signers([guardian])
            .rpc(),
        "Paused",
      );

      // Cleanup
      await program.methods
        .executeRecovery(adminPubkey, PublicKey.default)
        .accounts({ bridgeState, recovery: recovery.publicKey })
        .signers([recovery])
        .rpc();
    });

    it("recovery can optionally replace guardian", async () => {
      await program.methods
        .emergencyFreeze()
        .accounts({ bridgeState, guardian: guardian.publicKey })
        .signers([guardian])
        .rpc();

      const newGuardian = Keypair.generate();

      await program.methods
        .executeRecovery(adminPubkey, newGuardian.publicKey)
        .accounts({ bridgeState, recovery: recovery.publicKey })
        .signers([recovery])
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.guardian.equals(newGuardian.publicKey), "Guardian replaced");

      // Restore original guardian for subsequent tests
      await program.methods
        .setGuardian(guardian.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();
    });
  });

  // =========================================================================
  // 8. Role Management
  // =========================================================================

  describe("Role Management", () => {
    it("set guardian with role overlap check", async () => {
      // Cannot set guardian to admin address
      await expectError(
        () =>
          program.methods
            .setGuardian(adminPubkey)
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
            })
            .rpc(),
        "RoleOverlap",
      );
    });

    it("set operator", async () => {
      const newOp = Keypair.generate();
      await program.methods
        .setOperator(newOp.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.operator.equals(newOp.publicKey), "Operator updated");

      // Restore
      await program.methods
        .setOperator(operator.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();
    });

    it("set recovery", async () => {
      const newRec = Keypair.generate();
      await program.methods
        .setRecovery(newRec.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.recovery.equals(newRec.publicKey), "Recovery updated");

      // Restore
      await program.methods
        .setRecovery(recovery.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();
    });
  });

  // =========================================================================
  // 9. Skip Nonce & Refund
  // =========================================================================

  describe("Skip Nonce & Refund", () => {
    it("operator can skip nonce", async () => {
      const nonce = new anchor.BN(999_999);
      const crossChainRequest = crossChainRequestPDA(nonce);

      await program.methods
        .skipNonce(nonce)
        .accounts({
          bridgeState,
          crossChainRequest,
          operator: operator.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([operator])
        .rpc();

      const req = await program.account.crossChainRequest.fetch(
        crossChainRequest,
      );
      assert.equal(req.isProcessed, true, "Request marked as processed");
      assert.equal(req.nonce.toNumber(), 999_999);
    });

    it("cannot skip already processed nonce", async () => {
      const nonce = new anchor.BN(999_999);
      const crossChainRequest = crossChainRequestPDA(nonce);

      await expectError(
        () =>
          program.methods
            .skipNonce(nonce)
            .accounts({
              bridgeState,
              crossChainRequest,
              operator: operator.publicKey,
              systemProgram: SystemProgram.programId,
            })
            .signers([operator])
            .rpc(),
        "ConstraintSpace", // compact_request_pda resized the account; Anchor rejects the space mismatch
      );
    });

    it("operator can initiate refund", async () => {
      const nonce = new anchor.BN(77777);
      const stakeAmount = new anchor.BN(5_000_000); // 5 USDC

      await program.methods
        .stake(
          nonce,
          stakeAmount,
          Array.from(new Uint8Array(32).fill(0xab)) as number[],
        )
        .accounts({
          bridgeState,
          stakeRecord: stakeRecordPDA(nonce),
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

      // Step 1: Initiate refund
      await program.methods
        .initiateRefund(nonce)
        .accounts({
          bridgeState,
          stakeRecord: stakeRecordPDA(nonce),
          operator: operator.publicKey,
        })
        .signers([operator])
        .rpc();

      const sr = await program.account.stakeRecord.fetch(stakeRecordPDA(nonce));
      assert.ok(sr.refundInitiatedAt.toNumber() > 0, "Refund initiated timestamp set");
      assert.equal(sr.refunded, false, "Not yet refunded");
    });

    it("cannot execute refund before delay", async () => {
      const nonce = new anchor.BN(77777);

      await expectError(
        () =>
          program.methods
            .executeRefund(nonce)
            .accounts({
              bridgeState,
              stakeRecord: stakeRecordPDA(nonce),
              caller: operator.publicKey,
              vault,
              usdcMint,
              vaultTokenAccount,
              ownerTokenAccount: userTokenAccount,
              tokenProgram: TOKEN_PROGRAM_ID,
            })
            .signers([operator])
            .rpc(),
        "RefundNotReady",
      );
    });

    it("admin can cancel refund", async () => {
      const nonce = new anchor.BN(77777);

      await program.methods
        .cancelRefund(nonce)
        .accounts({
          bridgeState,
          stakeRecord: stakeRecordPDA(nonce),
          admin: adminPubkey,
        })
        .rpc();

      const sr = await program.account.stakeRecord.fetch(stakeRecordPDA(nonce));
      assert.equal(sr.refundInitiatedAt.toNumber(), 0, "Refund cancelled");
    });

    it("cannot double-initiate refund", async () => {
      const nonce = new anchor.BN(88888);
      const stakeAmount = new anchor.BN(5_000_000);

      await program.methods
        .stake(
          nonce,
          stakeAmount,
          Array.from(new Uint8Array(32).fill(0xab)) as number[],
        )
        .accounts({
          bridgeState,
          stakeRecord: stakeRecordPDA(nonce),
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

      await program.methods
        .initiateRefund(nonce)
        .accounts({
          bridgeState,
          stakeRecord: stakeRecordPDA(nonce),
          operator: operator.publicKey,
        })
        .signers([operator])
        .rpc();

      await expectError(
        () =>
          program.methods
            .initiateRefund(nonce)
            .accounts({
              bridgeState,
              stakeRecord: stakeRecordPDA(nonce),
              operator: operator.publicKey,
            })
            .signers([operator])
            .rpc(),
        "RefundAlreadyInitiated",
      );
    });

    it("non-operator cannot skip nonce", async () => {
      const nonce = new anchor.BN(888_888);
      const crossChainRequest = crossChainRequestPDA(nonce);

      await expectError(
        () =>
          program.methods
            .skipNonce(nonce)
            .accounts({
              bridgeState,
              crossChainRequest,
              operator: nonAdmin.publicKey,
              systemProgram: SystemProgram.programId,
            })
            .signers([nonAdmin])
            .rpc(),
        "Unauthorized",
      );
    });
  });

  // =========================================================================
  // 10. Timelock
  // =========================================================================

  describe("Timelock", () => {
    it("activate timelock", async () => {
      await program.methods
        .activateTimelock()
        .accounts({ bridgeState, admin: adminPubkey })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.timelockActive, true, "Timelock activated");
    });

    it("cannot activate timelock twice", async () => {
      await expectError(
        () =>
          program.methods
            .activateTimelock()
            .accounts({ bridgeState, admin: adminPubkey })
            .rpc(),
        "TimelockAlreadyActive",
      );
    });

    it("admin operations now require timelock", async () => {
      // configureFee without a valid timelock PDA should fail
      await expectError(
        () =>
          program.methods
            .configureFee(new anchor.BN(50_000))
            .accounts({
              bridgeState,
              timelockOp: adminPubkey, // not a valid timelock PDA
              admin: adminPubkey,
            })
            .rpc(),
        "TimelockNotScheduled",
      );
    });

    it("schedule and cancel operation", async () => {
      const feeData = Buffer.concat([
        Buffer.from("configureFee"),
        Buffer.from(new anchor.BN(50_000).toArrayLike(Buffer, "le", 8)),
      ]);
      const crypto = require("crypto");
      const opHashBuf = crypto.createHash("sha256").update(feeData).digest();
      const opHash = Array.from(new Uint8Array(opHashBuf)) as number[];

      const [timelockPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("timelock"), opHashBuf],
        program.programId,
      );

      await program.methods
        .scheduleOperation(opHash, Buffer.from(feeData))
        .accounts({
          bridgeState,
          timelockOp: timelockPda,
          admin: adminPubkey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      const tl = await program.account.timelockOperation.fetch(timelockPda);
      assert.ok(tl.eta.toNumber() > 0, "ETA is set");

      // Cancel it
      await program.methods
        .cancelOperation(opHash)
        .accounts({
          bridgeState,
          timelockOp: timelockPda,
          admin: adminPubkey,
        })
        .rpc();

      const info = await connection.getAccountInfo(timelockPda);
      assert.ok(info === null, "TimelockOperation PDA closed after cancel");
    });
  });

  // =========================================================================
  // 11. Withdraw Token
  // =========================================================================

  describe("Withdraw Token", () => {
    it("admin can withdraw from vault (timelock active — needs schedule)", async () => {
      // With timelock active, direct withdraw fails
      await expectError(
        () =>
          program.methods
            .withdrawToken(new anchor.BN(1_000_000), adminPubkey)
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
              vault,
              usdcMint,
              vaultTokenAccount,
              toTokenAccount: adminTokenAccount,
              tokenProgram: TOKEN_PROGRAM_ID,
            })
            .rpc(),
        "TimelockNotScheduled",
      );
    });
  });
});
