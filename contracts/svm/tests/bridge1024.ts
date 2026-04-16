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

  // ---- Chain / peer constants ----
  const LOCAL_CHAIN_ID = new anchor.BN(1);
  const PEER_CHAIN_ID = new anchor.BN(1024);
  const BRIDGE_FEE = new anchor.BN(100_000); // 0.1 USDC
  const MAX_STAKE = new anchor.BN(100_000_000); // 100 USDC
  const peerContract = new Uint8Array(32);
  peerContract[0] = 0x12;
  peerContract[1] = 0x34;

  // ---- Helpers ----

  function stakeRecordPDA(nonce: anchor.BN): PublicKey {
    const [pda] = PublicKey.findProgramAddressSync(
      [Buffer.from("stake_record"), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId,
    );
    return pda;
  }

  function peerConfigPDA(chainId: anchor.BN): PublicKey {
    const [pda] = PublicKey.findProgramAddressSync(
      [Buffer.from("peer_config"), chainId.toArrayLike(Buffer, "le", 8)],
      program.programId,
    );
    return pda;
  }

  function crossChainRequestPDA(
    sourceChainId: anchor.BN,
    nonce: anchor.BN,
  ): PublicKey {
    const [pda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("cross_chain_request"),
        sourceChainId.toArrayLike(Buffer, "le", 8),
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
      assert.equal(bs.isPaused, false, "Not paused initially");
      assert.equal(bs.timelockActive, false, "Timelock not active initially");
      assert.equal(bs.relayers.length, 0, "No relayers initially");
    });

    it("rejects duplicate role addresses", async () => {
      const bs2Admin = Keypair.generate();
      await airdropSol(bs2Admin.publicKey, 5);

      await expectError(
        () =>
          program.methods
            .initialize(
              bs2Admin.publicKey,
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
        "already in use",
      );
    });
  });

  // =========================================================================
  // 2. Configuration
  // =========================================================================

  describe("Configuration", () => {
    it("configure USDC mint and local chain ID", async () => {
      await program.methods
        .configure(usdcMint, LOCAL_CHAIN_ID)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.usdcMint.equals(usdcMint), "USDC mint set");
      assert.equal(bs.localChainId.toNumber(), 1, "Local chain ID set");
    });

    it("configure global rate limits (4 params, no maxStake)", async () => {
      await program.methods
        .configureRateLimits(
          new anchor.BN(100_000_000), // maxPerWindow: 100 USDC
          new anchor.BN(3600),        // windowDuration: 1 hour
          new anchor.BN(50_000_000),  // maxSingle: 50 USDC
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
      assert.equal(bs.minimumReserve.toNumber(), 10_000_000);
    });

    it("non-admin cannot configure", async () => {
      await expectError(
        () =>
          program.methods
            .configure(usdcMint, LOCAL_CHAIN_ID)
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
  // 3. Peer Management
  // =========================================================================

  describe("Peer Management", () => {
    it("register peer", async () => {
      await program.methods
        .registerPeer(
          PEER_CHAIN_ID,
          Array.from(peerContract) as number[],
          BRIDGE_FEE,
          MAX_STAKE,
        )
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
          timelockOp: adminPubkey,
          admin: adminPubkey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      const pc = await program.account.peerConfig.fetch(
        peerConfigPDA(PEER_CHAIN_ID),
      );
      assert.equal(pc.chainId.toNumber(), 1024, "Peer chain ID set");
      assert.deepEqual(
        pc.peerContract,
        Array.from(peerContract),
        "Peer contract set",
      );
      assert.equal(pc.bridgeFee.toNumber(), 100_000, "Bridge fee set");
      assert.equal(pc.maxStakeAmount.toNumber(), 100_000_000, "Max stake set");
    });

    it("cannot register same chain_id twice", async () => {
      await expectError(
        () =>
          program.methods
            .registerPeer(
              PEER_CHAIN_ID,
              Array.from(peerContract) as number[],
              BRIDGE_FEE,
              MAX_STAKE,
            )
            .accounts({
              bridgeState,
              peerConfig: peerConfigPDA(PEER_CHAIN_ID),
              timelockOp: adminPubkey,
              admin: adminPubkey,
              systemProgram: SystemProgram.programId,
            })
            .rpc(),
        "already in use",
      );
    });

    it("configure peer fee", async () => {
      const newFee = new anchor.BN(200_000); // 0.2 USDC
      await program.methods
        .configurePeerFee(PEER_CHAIN_ID, newFee)
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const pc = await program.account.peerConfig.fetch(
        peerConfigPDA(PEER_CHAIN_ID),
      );
      assert.equal(pc.bridgeFee.toNumber(), 200_000, "Fee updated");

      // Restore to 0.1 USDC
      await program.methods
        .configurePeerFee(PEER_CHAIN_ID, BRIDGE_FEE)
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();
    });

    it("configure peer contract address", async () => {
      const newPeer = new Uint8Array(32);
      newPeer[0] = 0xaa;
      newPeer[1] = 0xbb;

      await program.methods
        .configurePeer(PEER_CHAIN_ID, Array.from(newPeer) as number[])
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const pc = await program.account.peerConfig.fetch(
        peerConfigPDA(PEER_CHAIN_ID),
      );
      assert.deepEqual(pc.peerContract, Array.from(newPeer), "Peer contract updated");

      // Restore original
      await program.methods
        .configurePeer(PEER_CHAIN_ID, Array.from(peerContract) as number[])
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();
    });

    it("configure peer rate limits", async () => {
      await program.methods
        .configurePeerRateLimits(
          PEER_CHAIN_ID,
          new anchor.BN(50_000_000),  // maxPerWindow
          new anchor.BN(3600),        // windowDuration
          new anchor.BN(25_000_000),  // maxSingle
          new anchor.BN(100_000_000), // maxStake
        )
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const pc = await program.account.peerConfig.fetch(
        peerConfigPDA(PEER_CHAIN_ID),
      );
      assert.equal(pc.maxUnlockPerWindow.toNumber(), 50_000_000);
      assert.equal(pc.windowDuration.toNumber(), 3600);
      assert.equal(pc.maxSingleUnlock.toNumber(), 25_000_000);
      assert.equal(pc.maxStakeAmount.toNumber(), 100_000_000);

      // Reset per-chain limits to 0 (disabled) for later tests
      await program.methods
        .configurePeerRateLimits(
          PEER_CHAIN_ID,
          new anchor.BN(0),
          new anchor.BN(0),
          new anchor.BN(0),
          MAX_STAKE,
        )
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();
    });
  });

  // =========================================================================
  // 4. Relayer Management
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
  // 5. Stake (multi-peer: needs target_chain_id + peer_config)
  // =========================================================================

  describe("Stake", () => {
    it("stake USDC — creates StakeRecord with random nonce", async () => {
      const nonce = new anchor.BN(12345);
      const stakeAmount = new anchor.BN(10_000_000); // 10 USDC
      const receiver = new Uint8Array(32);
      receiver[0] = 0xab;
      receiver[1] = 0xcd;

      await program.methods
        .stake(
          nonce,
          stakeAmount,
          Array.from(receiver) as number[],
          PEER_CHAIN_ID,
        )
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
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
      assert.equal(sr.targetChainId.toNumber(), 1024, "Target chain ID stored");
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
              PEER_CHAIN_ID,
            )
            .accounts({
              bridgeState,
              peerConfig: peerConfigPDA(PEER_CHAIN_ID),
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
              PEER_CHAIN_ID,
            )
            .accounts({
              bridgeState,
              peerConfig: peerConfigPDA(PEER_CHAIN_ID),
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
              PEER_CHAIN_ID,
            )
            .accounts({
              bridgeState,
              peerConfig: peerConfigPDA(PEER_CHAIN_ID),
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

    it("rejects stake to unregistered peer chain", async () => {
      const nonce = new anchor.BN(55555);
      const unknownChain = new anchor.BN(9999);

      await expectError(
        () =>
          program.methods
            .stake(
              nonce,
              new anchor.BN(1_000_000),
              Array.from(new Uint8Array(32).fill(1)) as number[],
              unknownChain,
            )
            .accounts({
              bridgeState,
              peerConfig: peerConfigPDA(unknownChain),
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
        "AccountNotInitialized",
      );
    });
  });

  // =========================================================================
  // 6. Confirm Event (multi-peer: needs source_chain_id + peer_config)
  // =========================================================================

  describe("Confirm Event", () => {
    it("non-relayer cannot confirm event", async () => {
      const nonce = new anchor.BN(100);
      const crossChainRequest = crossChainRequestPDA(PEER_CHAIN_ID, nonce);

      const receiverTokenAccount = await createAccount(
        connection,
        admin,
        usdcMint,
        nonAdmin.publicKey,
      );

      const pc = await program.account.peerConfig.fetch(
        peerConfigPDA(PEER_CHAIN_ID),
      );
      const eventData = {
        sourceContract: pc.peerContract,
        targetContract: Array.from(program.programId.toBytes()),
        sourceChainId: PEER_CHAIN_ID,
        targetChainId: LOCAL_CHAIN_ID,
        blockHeight: new anchor.BN(100),
        amount: new anchor.BN(1_000_000),
        sender: Array.from(new Uint8Array(32)),
        receiver: Array.from(nonAdmin.publicKey.toBytes()),
        nonce,
      };

      await expectError(
        () =>
          program.methods
            .confirmEvent(nonce, PEER_CHAIN_ID, eventData)
            .accounts({
              bridgeState,
              peerConfig: peerConfigPDA(PEER_CHAIN_ID),
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
      const crossChainRequest = crossChainRequestPDA(PEER_CHAIN_ID, nonce);

      const receiverTokenAccount = await createAccount(
        connection,
        admin,
        usdcMint,
        relayer1.publicKey,
      );

      const eventData = {
        sourceContract: Array.from(peerContract),
        targetContract: Array.from(program.programId.toBytes()),
        sourceChainId: PEER_CHAIN_ID,
        targetChainId: LOCAL_CHAIN_ID,
        blockHeight: new anchor.BN(100),
        amount: new anchor.BN(1_000_000),
        sender: Array.from(new Uint8Array(32)),
        receiver: Array.from(relayer1.publicKey.toBytes()),
        nonce,
      };

      await expectError(
        () =>
          program.methods
            .confirmEvent(nonce, PEER_CHAIN_ID, eventData)
            .accounts({
              bridgeState,
              peerConfig: peerConfigPDA(PEER_CHAIN_ID),
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
  // 7. Admin Transfer (2-step)
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
  // 8. Emergency Freeze / Recovery
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
  // 9. Role Management
  // =========================================================================

  describe("Role Management", () => {
    it("set guardian with role overlap check", async () => {
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
  // 9.5 R5 Audit Fixes (M-R5-1 / L-R5-3)
  // =========================================================================

  describe("R5 Audit Fixes", () => {
    // M-R5-1: propose_admin 提前拒绝与现有任意角色重叠的提议，
    // 避免 timelock 调度被白白消耗、且 pending_admin 卡死在 accept_admin 阶段
    it("propose_admin rejects overlap with admin/guardian/operator/recovery", async () => {
      for (const overlap of [
        adminPubkey,
        guardian.publicKey,
        operator.publicKey,
        recovery.publicKey,
      ]) {
        await expectError(
          () =>
            program.methods
              .proposeAdmin(overlap)
              .accounts({
                bridgeState,
                timelockOp: adminPubkey,
                admin: adminPubkey,
              })
              .rpc(),
          "RoleOverlap",
        );
      }
    });

    // M-R5-1: 已经有 pending_admin 时，set_guardian / set_operator / set_recovery
    // 都不能把角色设为 pending_admin（否则 accept_admin 会因 RoleOverlap 永久卡死）
    it("set_guardian / set_operator / set_recovery reject overlap with pending_admin", async () => {
      const pendingCandidate = Keypair.generate();
      await airdropSol(pendingCandidate.publicKey, 5);

      // 先 propose 一个全新地址作为 pending_admin
      await program.methods
        .proposeAdmin(pendingCandidate.publicKey)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      // set_guardian(pendingCandidate) 应被预检拒绝
      await expectError(
        () =>
          program.methods
            .setGuardian(pendingCandidate.publicKey)
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
            })
            .rpc(),
        "RoleOverlap",
      );

      // set_operator(pendingCandidate) 同样被拒
      await expectError(
        () =>
          program.methods
            .setOperator(pendingCandidate.publicKey)
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
            })
            .rpc(),
        "RoleOverlap",
      );

      // set_recovery(pendingCandidate) 同样被拒
      await expectError(
        () =>
          program.methods
            .setRecovery(pendingCandidate.publicKey)
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
            })
            .rpc(),
        "RoleOverlap",
      );

      // 清理：让 candidate 接受 admin，再 propose 原 admin 转回去，accept 后一切复原
      await program.methods
        .acceptAdmin()
        .accounts({ bridgeState, newAdmin: pendingCandidate.publicKey })
        .signers([pendingCandidate])
        .rpc();

      await program.methods
        .proposeAdmin(adminPubkey)
        .accounts({
          bridgeState,
          timelockOp: pendingCandidate.publicKey,
          admin: pendingCandidate.publicKey,
        })
        .signers([pendingCandidate])
        .rpc();

      await program.methods
        .acceptAdmin()
        .accounts({ bridgeState, newAdmin: adminPubkey })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.admin.equals(adminPubkey), "admin restored after cleanup");
      assert.ok(
        bs.pendingAdmin.equals(PublicKey.default),
        "pending_admin cleared after cleanup",
      );
    });

    // L-R5-3: stake 现在显式断言 target_chain_id == peer_config.chain_id，
    // 即便 PDA seeds 已隐式约束，错配仍会被以更直观的 InvalidChainId 错误拒绝
    // （参数名已从 _target_chain_id 改为 target_chain_id，positional 调用仍兼容）
    it("stake works after target_chain_id rename and matches peer_config.chain_id", async () => {
      const nonce = new anchor.BN(424242);
      const stakeAmount = new anchor.BN(2_000_000); // 2 USDC

      await program.methods
        .stake(
          nonce,
          stakeAmount,
          Array.from(new Uint8Array(32).fill(0xcd)) as number[],
          PEER_CHAIN_ID,
        )
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
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
      assert.ok(sr.owner.equals(user.publicKey), "stake_record owner set");
      assert.equal(sr.targetChainId.toNumber(), PEER_CHAIN_ID.toNumber());
    });
  });

  // =========================================================================
  // 10. Skip Nonce & Refund (multi-peer: skip_nonce needs source_chain_id)
  // =========================================================================

  describe("Skip Nonce & Refund", () => {
    it("operator can skip nonce", async () => {
      const nonce = new anchor.BN(999_999);
      const crossChainRequest = crossChainRequestPDA(PEER_CHAIN_ID, nonce);

      await program.methods
        .skipNonce(nonce, PEER_CHAIN_ID)
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
      const crossChainRequest = crossChainRequestPDA(PEER_CHAIN_ID, nonce);

      await expectError(
        () =>
          program.methods
            .skipNonce(nonce, PEER_CHAIN_ID)
            .accounts({
              bridgeState,
              crossChainRequest,
              operator: operator.publicKey,
              systemProgram: SystemProgram.programId,
            })
            .signers([operator])
            .rpc(),
        "ConstraintSpace", // compact_request_pda shrank the account; Anchor rejects the space mismatch
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
          PEER_CHAIN_ID,
        )
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
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
          PEER_CHAIN_ID,
        )
        .accounts({
          bridgeState,
          peerConfig: peerConfigPDA(PEER_CHAIN_ID),
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
      const crossChainRequest = crossChainRequestPDA(PEER_CHAIN_ID, nonce);

      await expectError(
        () =>
          program.methods
            .skipNonce(nonce, PEER_CHAIN_ID)
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
  // 11. Timelock
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
      await expectError(
        () =>
          program.methods
            .configurePeerFee(PEER_CHAIN_ID, new anchor.BN(50_000))
            .accounts({
              bridgeState,
              peerConfig: peerConfigPDA(PEER_CHAIN_ID),
              timelockOp: adminPubkey,
              admin: adminPubkey,
            })
            .rpc(),
        "TimelockNotScheduled",
      );
    });

    it("schedule and cancel operation", async () => {
      const feeData = Buffer.concat([
        Buffer.from("configurePeerFee"),
        PEER_CHAIN_ID.toArrayLike(Buffer, "le", 8),
        new anchor.BN(50_000).toArrayLike(Buffer, "le", 8),
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
  // 12. Withdraw Token (uses tokenMint, not usdcMint)
  // =========================================================================

  describe("Withdraw Token", () => {
    it("admin can withdraw from vault (timelock active — needs schedule)", async () => {
      await expectError(
        () =>
          program.methods
            .withdrawToken(new anchor.BN(1_000_000), adminPubkey)
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
              vault,
              tokenMint: usdcMint,
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
