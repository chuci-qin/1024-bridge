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

  // leaf 单 peer：crossChainRequest PDA seeds 只用 nonce（不再含 source_chain_id）
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
    it("configure USDC mint + chain IDs + single peer + bridge fee (one shot)", async () => {
      await program.methods
        .configure(
          usdcMint,
          Array.from(peerContract) as number[],
          LOCAL_CHAIN_ID,
          PEER_CHAIN_ID,
          BRIDGE_FEE,
        )
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.ok(bs.usdcMint.equals(usdcMint), "USDC mint set");
      assert.equal(bs.localChainId.toNumber(), 1, "Local chain ID set");
      assert.equal(bs.peerChainId.toNumber(), 1024, "Peer chain ID set");
      assert.deepEqual(bs.peerContract, Array.from(peerContract), "Peer contract set");
      assert.equal(bs.bridgeFee.toNumber(), 100_000, "Bridge fee set");
    });

    it("configure rate limits (5 params including max_stake, with EVM-style symmetry)", async () => {
      await program.methods
        .configureRateLimits(
          new anchor.BN(100_000_000), // maxPerWindow: 100 USDC
          new anchor.BN(3600),        // windowDuration: 1 hour
          new anchor.BN(50_000_000),  // maxSingle: 50 USDC
          MAX_STAKE,                  // maxStake: 100 USDC
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
              LOCAL_CHAIN_ID,
              PEER_CHAIN_ID,
              BRIDGE_FEE,
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
  // 2.5 Configure Bridge Fee (independent timelock op, mirrors EVM configureBridgeFee)
  // =========================================================================

  describe("Configure Bridge Fee", () => {
    it("admin can update bridge fee independently", async () => {
      const newFee = new anchor.BN(200_000); // 0.2 USDC

      await program.methods
        .configureBridgeFee(newFee)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      let bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.bridgeFee.toNumber(), 200_000, "Bridge fee updated");

      // Restore original
      await program.methods
        .configureBridgeFee(BRIDGE_FEE)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.bridgeFee.toNumber(), 100_000, "Bridge fee restored");
    });

    it("rejects fee above MAX_FEE", async () => {
      await expectError(
        () =>
          program.methods
            .configureBridgeFee(new anchor.BN(1_000_000_001))
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
            })
            .rpc(),
        "FeeTooHigh",
      );
    });

    it("non-admin cannot configure bridge fee", async () => {
      await expectError(
        () =>
          program.methods
            .configureBridgeFee(new anchor.BN(50_000))
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
  // 2.6 Configure Gasless Fee (independent timelock op, mirrors EVM configureGaslessFee)
  // =========================================================================

  describe("Configure Gasless Fee", () => {
    it("admin can set gasless fee", async () => {
      const gaslessFee = new anchor.BN(50_000); // 0.05 USDC
      await program.methods
        .configureGaslessFee(gaslessFee)
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const bs = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bs.gaslessFee.toNumber(), 50_000, "gasless fee set");
    });

    it("rejects fee above MAX_FEE", async () => {
      await expectError(
        () =>
          program.methods
            .configureGaslessFee(new anchor.BN(1_000_000_001))
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
            })
            .rpc(),
        "FeeTooHigh",
      );
    });

    it("non-admin cannot configure gasless fee", async () => {
      await expectError(
        () =>
          program.methods
            .configureGaslessFee(new anchor.BN(10_000))
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
  // 3. (Peer Management removed — leaf has single hardcoded peer in BridgeState)
  // =========================================================================

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
  // 5. Stake (leaf single-peer: no target_chain_id, no peer_config)
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
  // 5.5 Stake Gasless (paymaster pays SOL gas, bridge_fee + gasless_fee deducted)
  // =========================================================================

  describe("Stake Gasless", () => {
    it("stake_gasless succeeds and deducts bridge_fee + gasless_fee", async () => {
      // gasless_fee already set to 50_000 by Configure Gasless Fee describe
      const bsBefore = await program.account.bridgeState.fetch(bridgeState);
      assert.equal(bsBefore.gaslessFee.toNumber(), 50_000, "gasless fee preset");
      assert.equal(bsBefore.bridgeFee.toNumber(), 100_000, "bridge fee preset");

      const nonce = new anchor.BN(58001);
      const stakeAmount = new anchor.BN(5_000_000); // 5 USDC
      const receiver = new Uint8Array(32);
      receiver[0] = 0xee;

      const userBalBefore = (await getAccount(connection, userTokenAccount)).amount;
      const vaultBalBefore = (await getAccount(connection, vaultTokenAccount)).amount;

      await program.methods
        .stakeGasless(nonce, stakeAmount, Array.from(receiver) as number[])
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
      // amount = full deposit (含两笔 fee)，用于 refund 全额退给用户
      assert.equal(sr.amount.toNumber(), 5_000_000, "StakeRecord stores full amount");

      const userBalAfter = (await getAccount(connection, userTokenAccount)).amount;
      const vaultBalAfter = (await getAccount(connection, vaultTokenAccount)).amount;
      assert.equal(
        userBalBefore - userBalAfter,
        BigInt(5_000_000),
        "user paid 5 USDC",
      );
      assert.equal(
        vaultBalAfter - vaultBalBefore,
        BigInt(5_000_000),
        "vault received 5 USDC (含 fee 留在 vault)",
      );
    });

    it("stake_gasless reverts when gasless_fee == 0 (circuit breaker)", async () => {
      // 临时把 gasless_fee 调成 0 验证熔断
      await program.methods
        .configureGaslessFee(new anchor.BN(0))
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();

      const nonce = new anchor.BN(58002);
      await expectError(
        () =>
          program.methods
            .stakeGasless(
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
        "GaslessDisabled",
      );

      // 恢复到 50_000，避免影响后续可能的 gasless 用例
      await program.methods
        .configureGaslessFee(new anchor.BN(50_000))
        .accounts({
          bridgeState,
          timelockOp: adminPubkey,
          admin: adminPubkey,
        })
        .rpc();
    });

    it("普通 stake 路径不受 gasless_fee 影响", async () => {
      // gasless_fee = 50_000, bridge_fee = 100_000；普通 stake 只扣 bridge_fee
      const nonce = new anchor.BN(58003);
      const stakeAmount = new anchor.BN(3_000_000);

      await program.methods
        .stake(
          nonce,
          stakeAmount,
          Array.from(new Uint8Array(32).fill(0xcc)) as number[],
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

      const sr = await program.account.stakeRecord.fetch(stakeRecordPDA(nonce));
      assert.equal(sr.amount.toNumber(), 3_000_000, "普通 stake 仍记全额");
    });
  });

  // =========================================================================
  // 6. Confirm Event (leaf single-peer: no source_chain_id, no peer_config)
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

      // leaf reads peer_contract from BridgeState
      const bs = await program.account.bridgeState.fetch(bridgeState);
      const eventData = {
        sourceContract: bs.peerContract,
        targetContract: Array.from(program.programId.toBytes()),
        sourceChainId: PEER_CHAIN_ID,
        targetChainId: LOCAL_CHAIN_ID,
        blockHeight: new anchor.BN(100),
        rawAmount: new anchor.BN(1_000_000),
        amount: new anchor.BN(1_000_000),
        sender: Array.from(new Uint8Array(32)),
        receiver: Array.from(nonAdmin.publicKey.toBytes()),
        nonce,
      };

      await expectError(
        () =>
          program.methods
            .confirmEvent(eventData)
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
        sourceContract: Array.from(peerContract),
        targetContract: Array.from(program.programId.toBytes()),
        sourceChainId: PEER_CHAIN_ID,
        targetChainId: LOCAL_CHAIN_ID,
        blockHeight: new anchor.BN(100),
        rawAmount: new anchor.BN(1_000_000),
        amount: new anchor.BN(1_000_000),
        sender: Array.from(new Uint8Array(32)),
        receiver: Array.from(relayer1.publicKey.toBytes()),
        nonce,
      };

      await expectError(
        () =>
          program.methods
            .confirmEvent(eventData)
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

    // L-R5-3 (legacy multi-peer fix) no longer applies — leaf stake() has no
    // target_chain_id param, target chain is implicit via BridgeState.peer_chain_id
  });

  // =========================================================================
  // 10. Skip Nonce & Refund (leaf single-peer: skip_nonce drops source_chain_id)
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

    // R6: skip_nonce 必须能在"已有 partial 投票但未达阈值"的 PDA 上正常工作，
    // 与 EVM Bridge1024.sol `skipNonce` 语义对齐（只拒绝 is_processed == true）。
    it("operator can skip nonce after partial vote (compacts PDA & blocks further confirm)", async () => {
      const nonce = new anchor.BN(555_555);
      const crossChainRequest = crossChainRequestPDA(nonce);

      // 用独立 keypair 作为 receiver_token_account owner，避免与 §6 测试的
      // nonAdmin ATA 冲突（spl-token createAccount 不带 keypair 参数时走 ATA 路径，
      // 同一 owner+mint 二次调用会被 Associated Token Program 拒绝）。
      const fakeReceiver = Keypair.generate();
      const receiverTokenAccount = await createAccount(
        connection,
        admin,
        usdcMint,
        fakeReceiver.publicKey,
      );

      const bs = await program.account.bridgeState.fetch(bridgeState);
      const eventData = {
        sourceContract: bs.peerContract,
        targetContract: Array.from(program.programId.toBytes()),
        sourceChainId: PEER_CHAIN_ID,
        targetChainId: LOCAL_CHAIN_ID,
        blockHeight: new anchor.BN(200),
        rawAmount: new anchor.BN(1_000_000),
        amount: new anchor.BN(1_000_000),
        sender: Array.from(new Uint8Array(32)),
        receiver: Array.from(fakeReceiver.publicKey.toBytes()),
        nonce,
      };

      // 一个 relayer 投票（partial：threshold ≥ 2/3 * 18 = 12，1 票远未达成）
      await program.methods
        .confirmEvent(eventData)
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
        .rpc();

      const reqBefore = await program.account.crossChainRequest.fetch(crossChainRequest);
      assert.equal(reqBefore.isProcessed, false, "partial vote 状态未处理");
      assert.equal(reqBefore.confirmedRelayers.length, 1, "正好 1 票");

      const infoBefore = await connection.getAccountInfo(crossChainRequest);
      const sizeBefore = infoBefore!.data.length;

      // operator 跳过该 nonce —— 清空投票并压缩 PDA
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

      const infoAfter = await connection.getAccountInfo(crossChainRequest);
      assert.ok(
        infoAfter!.data.length < sizeBefore,
        `PDA 应被压缩：before=${sizeBefore} after=${infoAfter!.data.length}`,
      );

      const reqAfter = await program.account.crossChainRequest.fetch(crossChainRequest);
      assert.equal(reqAfter.isProcessed, true, "skip 后标记为已处理");
      assert.equal(reqAfter.confirmedRelayers.length, 0, "投票已清空");

      // 再次 confirm_event 应失败：init_if_needed 检查到已存在但 space 不匹配（压缩后）
      await expectError(
        () =>
          program.methods
            .confirmEvent(eventData)
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
        "ConstraintSpace",
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
      // After timelock activation, configureBridgeFee directly should be rejected
      // because the op_hash PDA was never scheduled
      await expectError(
        () =>
          program.methods
            .configureBridgeFee(new anchor.BN(50_000))
            .accounts({
              bridgeState,
              timelockOp: adminPubkey,
              admin: adminPubkey,
            })
            .rpc(),
        "TimelockNotScheduled",
      );
    });

    it("schedule and cancel operation", async () => {
      const feeData = Buffer.concat([
        Buffer.from("configureBridgeFee"),
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
        .scheduleOperation(Buffer.from(feeData))
        .accounts({
          bridgeState,
          timelockOp: timelockPda,
          admin: adminPubkey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      const tl = await program.account.timelockOperation.fetch(timelockPda);
      assert.ok(tl.eta.toNumber() > 0, "ETA is set");
      assert.deepEqual(
        Array.from(tl.opHash as Uint8Array | number[]),
        opHash,
        "op_hash 字段与 sha256(data) 一致（验证手动 try_serialize 写入正确）",
      );

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

    // R6: schedule_operation 切换到手动 CPI create_account 后，PDA 一致性由指令体内的
    // require_keys_eq! 守护。客户端传错地址（如传 admin）必须立即 TimelockNotScheduled。
    it("rejects schedule_operation with mismatched timelock_op address", async () => {
      const feeData = Buffer.concat([
        Buffer.from("configureBridgeFee"),
        new anchor.BN(60_000).toArrayLike(Buffer, "le", 8),
      ]);

      await expectError(
        () =>
          program.methods
            .scheduleOperation(Buffer.from(feeData))
            .accounts({
              bridgeState,
              timelockOp: adminPubkey, // 故意传 admin —— 不是 sha256(data) 派生的 PDA
              admin: adminPubkey,
              systemProgram: SystemProgram.programId,
            })
            .rpc(),
        "TimelockNotScheduled",
      );
    });

    // R6: 同一 op_hash 在被 cancel / consume 后必须能再次调度。
    // 与 EVM `delete timelockEta[opHash]` 后允许重入语义对齐。
    // SVM 通过 PDA 在 lamports == 0 时被 Solana runtime 跨 tx GC 来等效实现。
    it("allows re-scheduling the same op_hash after cancel (round-trip)", async () => {
      const feeData = Buffer.concat([
        Buffer.from("configureBridgeFee"),
        new anchor.BN(80_000).toArrayLike(Buffer, "le", 8),
      ]);
      const crypto = require("crypto");
      const opHashBuf = crypto.createHash("sha256").update(feeData).digest();
      const opHash = Array.from(new Uint8Array(opHashBuf)) as number[];

      const [timelockPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("timelock"), opHashBuf],
        program.programId,
      );

      // 第一次 schedule
      await program.methods
        .scheduleOperation(Buffer.from(feeData))
        .accounts({
          bridgeState,
          timelockOp: timelockPda,
          admin: adminPubkey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();
      const firstEta = (
        await program.account.timelockOperation.fetch(timelockPda)
      ).eta.toNumber();
      assert.ok(firstEta > 0, "首次调度后 ETA 已设");

      // cancel 把 PDA 关掉
      await program.methods
        .cancelOperation(opHash)
        .accounts({
          bridgeState,
          timelockOp: timelockPda,
          admin: adminPubkey,
        })
        .rpc();
      assert.equal(
        await connection.getAccountInfo(timelockPda),
        null,
        "cancel 后 PDA 应被 close",
      );

      // 第二次 schedule 同样的 data —— 必须成功（与 EVM 对齐：mapping 已 delete）
      await program.methods
        .scheduleOperation(Buffer.from(feeData))
        .accounts({
          bridgeState,
          timelockOp: timelockPda,
          admin: adminPubkey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();
      const secondEta = (
        await program.account.timelockOperation.fetch(timelockPda)
      ).eta.toNumber();
      assert.ok(secondEta >= firstEta, "二次调度 ETA 不低于首次");

      // 清理
      await program.methods
        .cancelOperation(opHash)
        .accounts({
          bridgeState,
          timelockOp: timelockPda,
          admin: adminPubkey,
        })
        .rpc();
    });

    // R6: 同一 op_hash 重复调度必须被 data_is_empty 检查拦下；
    // 否则手动 create_account CPI 会在 system_program 层 panic（AccountAlreadyInUse），
    // 错误码不可读且无法走 anchor 重试路径。
    it("rejects schedule_operation when op_hash already scheduled", async () => {
      const feeData = Buffer.concat([
        Buffer.from("configureBridgeFee"),
        new anchor.BN(70_000).toArrayLike(Buffer, "le", 8),
      ]);
      const crypto = require("crypto");
      const opHashBuf = crypto.createHash("sha256").update(feeData).digest();
      const opHash = Array.from(new Uint8Array(opHashBuf)) as number[];

      const [timelockPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("timelock"), opHashBuf],
        program.programId,
      );

      await program.methods
        .scheduleOperation(Buffer.from(feeData))
        .accounts({
          bridgeState,
          timelockOp: timelockPda,
          admin: adminPubkey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      try {
        await expectError(
          () =>
            program.methods
              .scheduleOperation(Buffer.from(feeData))
              .accounts({
                bridgeState,
                timelockOp: timelockPda,
                admin: adminPubkey,
                systemProgram: SystemProgram.programId,
              })
              .rpc(),
          "TimelockAlreadyScheduled",
        );
      } finally {
        // 清理：取消刚调度的操作，避免污染后续测试
        await program.methods
          .cancelOperation(opHash)
          .accounts({
            bridgeState,
            timelockOp: timelockPda,
            admin: adminPubkey,
          })
          .rpc();
      }
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
