// SVM 跨链桥程序集成测试套件，基于 Anchor 框架编写，覆盖初始化、配置、中继器管理、
// 质押、签名提交、管理员转移、暂停/恢复、关闭请求和流动性管理等核心功能
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

// 顶层测试组：bridge1024 跨链桥全部功能的集成测试
describe("bridge1024", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.Bridge1024 as Program<Bridge1024>;
  const connection = provider.connection;
  const admin = (provider.wallet as anchor.Wallet).payer;
  const adminPubkey = admin.publicKey;

  // ---- PDAs ----
  // senderState: 发送方状态 PDA，存储管理员、nonce、暂停标志等发送端配置
  const [senderState] = PublicKey.findProgramAddressSync(
    [Buffer.from("sender_state")],
    program.programId,
  );
  // receiverState: 接收方状态 PDA，存储中继器列表、手续费、接收端配置
  const [receiverState] = PublicKey.findProgramAddressSync(
    [Buffer.from("receiver_state")],
    program.programId,
  );
  // vault: 金库 PDA，作为跨链桥的 USDC 代币托管账户的权限拥有者
  const [vault] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault")],
    program.programId,
  );

  // ---- Keypairs ----
  // 测试用密钥对：普通用户、非管理员、新管理员以及三个中继器角色
  const user = Keypair.generate();
  const nonAdmin = Keypair.generate();
  const newAdmin = Keypair.generate();
  const relayer1 = Keypair.generate();
  const relayer2 = Keypair.generate();
  const relayer3 = Keypair.generate();

  // ---- Token state ----
  // 代币相关状态：USDC 铸币地址、错误铸币地址（用于负面测试），以及各代币账户
  let usdcMint: PublicKey;
  let wrongMint: PublicKey;
  let vaultTokenAccount: PublicKey;
  let adminTokenAccount: PublicKey;
  let userTokenAccount: PublicKey;
  let userWrongMintAccount: PublicKey;

  const USDC_DECIMALS = 6;
  const INITIAL_USDC = 1_000_000_000; // 1 000 USDC

  // ---- Helpers ----

  // 根据 nonce 值派生跨链请求 PDA 地址，用于定位链上跨链请求记录
  function crossChainRequestPDA(nonce: anchor.BN): PublicKey {
    const [pda] = PublicKey.findProgramAddressSync(
      [Buffer.from("cross_chain_request"), nonce.toArrayLike(Buffer, "le", 8)],
      program.programId,
    );
    return pda;
  }

  // 断言辅助函数：执行给定操作并验证其抛出包含预期错误信息的异常
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

  // 向指定地址空投 SOL，用于支付测试中的交易手续费
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

  // 测试环境初始化：为测试钱包空投 SOL，创建 USDC 和错误铸币，
  // 创建金库、管理员和用户的代币账户，并向用户和管理员铸造初始 USDC
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

  // 初始化测试组：验证跨链桥状态的首次初始化是否正确设置了管理员、金库、nonce 和暂停标志
  describe("Initialization", () => {
    // 验证 initialize 指令能正确初始化 senderState 和 receiverState，
    // 包括管理员为部署者、金库 PDA 被保存、nonce 从 0 开始、初始未暂停
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

  // 配置测试组：验证管理员能配置 USDC 铸币地址、对等链信息和手续费，以及非管理员被拒绝
  describe("Configuration", () => {
    // 验证 configureUsdc 指令能同时在 senderState 和 receiverState 中设置 USDC 铸币地址
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

    // 验证 configurePeer 指令设置对等合约地址和链 ID，且 receiverState 中的
    // sourceChainId 和 targetChainId 与 senderState 互换（因为收发方向相反）
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

    // 验证管理员可以设置跨链桥手续费（此处设为 0.1 USDC）
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

    // 验证非管理员调用配置指令时会被拒绝（返回 Unauthorized 错误）
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

  // 中继器管理测试组：验证添加、去重、轮换、移除中继器，最大数量限制，以及非管理员被拒绝
  describe("Relayer Management", () => {
    // 验证管理员可以成功添加中继器，且 relayerCount 递增
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

    // 验证重复添加同一中继器时会被拒绝（返回 RelayerAlreadyExists 错误）
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

    // 验证中继器轮换：将 relayer2 替换为 relayer3，轮换后 relayerCount 不变
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

    // 验证管理员可以移除中继器，且 relayerCount 递减
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

    // 验证中继器数量达到上限（MAX_RELAYERS = 18）后，再添加会返回 TooManyRelayers 错误
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

    // 验证非管理员无法添加中继器（返回 Unauthorized 错误）
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

  // 质押测试组：验证用户质押 USDC 的核心流程（nonce 递增、金库余额增加），
  // 以及暂停状态阻止质押和错误铸币被拒绝的边界情况
  describe("Stake", () => {
    // 验证质押成功后 nonce 递增 1，且金库代币账户余额至少增加了质押金额
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

    // 验证暂停状态下质押操作被拒绝（返回 Paused 错误），测试后恢复非暂停状态
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

    // 验证使用未配置的铸币地址进行质押时被拒绝（返回 UsdcNotConfigured 错误）
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

  // 签名提交测试组：由于完整的 Ed25519 签名验证需要专门的测试工具链（见下方 TODO），
  // 此处仅测试非中继器被拒绝和暂停状态阻止提交两个守卫条件
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

    // 验证非中继器身份提交签名时被拒绝（返回 RelayerNotFound 错误）
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

    // 验证暂停状态下中继器提交签名被拒绝（返回 Paused 错误），测试后恢复非暂停状态
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

  // 管理员转移测试组：验证两步式管理员转移流程（提议 → 接受），
  // 非待定账户无法接受转移，以及转移后恢复原管理员
  describe("Admin Transfer", () => {
    // 验证完整的两步管理员转移：先 proposeAdmin 设置 pendingAdmin，
    // 再由新管理员调用 acceptAdmin 完成转移，最后恢复原管理员以供后续测试使用
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

    // 验证非待定管理员（nonAdmin）无法接受管理员转移（返回 Unauthorized 错误），
    // 然后由正确的待定管理员完成接受并恢复原管理员
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

  // 暂停/恢复测试组：验证管理员可以暂停和恢复跨链桥，非管理员操作被拒绝
  describe("Pause / Unpause", () => {
    // 验证管理员调用 pause 后 senderState 和 receiverState 均为暂停状态，
    // 调用 unpause 后均恢复为非暂停状态
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

    // 验证非管理员调用 pause 时被拒绝（返回 Unauthorized 错误）
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

  // 关闭请求测试组：由于缺少 Ed25519 测试工具链无法创建已解锁的跨链请求 PDA，
  // 此处仅验证关闭不存在的请求 PDA 时 Anchor 在账户反序列化层即拒绝
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

    // 验证尝试关闭不存在的跨链请求 PDA 时返回 AccountNotInitialized 错误
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

  // 流动性管理测试组：验证管理员可以向金库添加和提取流动性，并校验余额变化
  describe("Liquidity Management", () => {
    // 验证 addLiquidity 后金库余额增加指定金额，withdrawLiquidity 后金库余额减少且
    // 管理员代币账户收到提取的代币
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
