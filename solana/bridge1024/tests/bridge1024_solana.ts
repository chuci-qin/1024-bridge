import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { assert, expect } from "chai";

/**
 * Test suite for Solana Bridge program (sender side for Solana→1024chain).
 *
 * TDD: these tests define expected behavior. Implementation in lib.rs
 * must pass all of them.
 *
 * Test IDs correspond to docs/features/solana-1024-bridge.md
 */
describe("bridge1024_solana", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Bridge1024Solana as Program;
  const admin = provider.wallet.payer;

  let usdcMint: PublicKey;
  let adminTokenAccount: PublicKey;
  let userTokenAccount: PublicKey;
  let vaultTokenAccount: PublicKey;
  let vault: PublicKey;
  let senderState: PublicKey;
  let bridgeConfig: PublicKey;

  const user = Keypair.generate();

  before(async () => {
    // Airdrop SOL to user
    const sig = await provider.connection.requestAirdrop(
      user.publicKey,
      2 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);

    // Derive PDAs
    [senderState] = PublicKey.findProgramAddressSync(
      [Buffer.from("sender_state")],
      program.programId
    );
    [bridgeConfig] = PublicKey.findProgramAddressSync(
      [Buffer.from("bridge_config")],
      program.programId
    );
    [vault] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault")],
      program.programId
    );

    // Create USDC mock mint
    usdcMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID
    );

    // Create token accounts
    adminTokenAccount = await createAccount(
      provider.connection,
      admin,
      usdcMint,
      admin.publicKey,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID
    );

    userTokenAccount = await createAccount(
      provider.connection,
      admin,
      usdcMint,
      user.publicKey,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID
    );

    vaultTokenAccount = await createAccount(
      provider.connection,
      admin,
      usdcMint,
      vault,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID
    );

    // Mint USDC to user (1000 USDC)
    await mintTo(
      provider.connection,
      admin,
      usdcMint,
      userTokenAccount,
      admin.publicKey,
      1_000_000_000,
      [],
      undefined,
      TOKEN_PROGRAM_ID
    );
  });

  // ===== SOL-T001: Initialize =====
  it("SOL-T001: initializes bridge program with defaults", async () => {
    await program.methods
      .initialize()
      .accounts({
        senderState,
        bridgeConfig,
        admin: admin.publicKey,
        vault,
        systemProgram: SystemProgram.programId,
      })
      .rpc();

    const state = await program.account.senderState.fetch(senderState);
    assert.equal(state.nonce.toNumber(), 0);
    assert.deepEqual(state.vault, vault);
    assert.deepEqual(state.admin, admin.publicKey);
    assert.deepEqual(state.usdcMint, PublicKey.default);

    const config = await program.account.bridgeConfig.fetch(bridgeConfig);
    assert.equal(config.bridgeFee.toNumber(), 0);
    assert.deepEqual(config.admin, admin.publicKey);
  });

  // ===== SOL-T002: Configure USDC =====
  it("SOL-T002: configure_usdc sets USDC mint", async () => {
    await program.methods
      .configureUsdc(usdcMint)
      .accounts({
        senderState,
        admin: admin.publicKey,
      })
      .rpc();

    const state = await program.account.senderState.fetch(senderState);
    assert.deepEqual(state.usdcMint, usdcMint);
  });

  // ===== SOL-T003: Configure Peer =====
  it("SOL-T003: configure_peer sets target chain info", async () => {
    const targetContract = "7KuLUKPqx6MymPJBi6CAUchg9uUUrL8PaoWK6hgFc93E";
    const sourceChainId = new anchor.BN(1); // Solana
    const targetChainId = new anchor.BN(91024); // 1024chain

    await program.methods
      .configurePeer(targetContract, sourceChainId, targetChainId)
      .accounts({
        senderState,
        admin: admin.publicKey,
      })
      .rpc();

    const state = await program.account.senderState.fetch(senderState);
    assert.equal(state.targetContract, targetContract);
    assert.equal(state.sourceChainId.toNumber(), 1);
    assert.equal(state.targetChainId.toNumber(), 91024);
  });

  // ===== SOL-T004: Configure Fee =====
  it("SOL-T004: configure_fee sets bridge_fee", async () => {
    const fee = new anchor.BN(5_000_000); // 5 USDC

    await program.methods
      .configureFee(fee)
      .accounts({
        bridgeConfig,
        admin: admin.publicKey,
      })
      .rpc();

    const config = await program.account.bridgeConfig.fetch(bridgeConfig);
    assert.equal(config.bridgeFee.toNumber(), 5_000_000);
  });

  // ===== SOL-T005: Stake transfers tokens to vault =====
  it("SOL-T005: stake() transfers tokens to vault", async () => {
    const amount = new anchor.BN(10_000_000); // 10 USDC
    const receiverAddress = "CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC";

    const userBefore = await getAccount(provider.connection, userTokenAccount);
    const vaultBefore = await getAccount(provider.connection, vaultTokenAccount);

    await program.methods
      .stake(amount, receiverAddress)
      .accounts({
        senderState,
        bridgeConfig,
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

    const userAfter = await getAccount(provider.connection, userTokenAccount);
    const vaultAfter = await getAccount(provider.connection, vaultTokenAccount);

    // Full amount transferred to vault (fee stays in vault)
    assert.equal(
      Number(vaultAfter.amount - vaultBefore.amount),
      10_000_000
    );
    assert.equal(
      Number(userBefore.amount - userAfter.amount),
      10_000_000
    );
  });

  // ===== SOL-T006: Stake emits StakeEvent with net_amount =====
  it("SOL-T006: stake() emits StakeEvent with net_amount (amount - fee)", async () => {
    // bridge_fee is 5 USDC, stake 10 USDC => net_amount = 5 USDC
    const amount = new anchor.BN(10_000_000);
    const receiverAddress = "CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC";

    const listener = program.addEventListener("StakeEvent", (event: any) => {
      assert.equal(event.amount.toNumber(), 5_000_000); // net_amount = 10M - 5M fee
      assert.equal(event.receiverAddress, receiverAddress);
      assert.ok(event.sender.length > 0);
      assert.ok(event.nonce.toNumber() > 0);
    });

    await program.methods
      .stake(amount, receiverAddress)
      .accounts({
        senderState,
        bridgeConfig,
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

    // Wait for event
    await new Promise((resolve) => setTimeout(resolve, 1000));
    program.removeEventListener(listener);
  });

  // ===== SOL-T007: Nonce increments =====
  it("SOL-T007: stake() increments nonce", async () => {
    const stateBefore = await program.account.senderState.fetch(senderState);
    const nonceBefore = stateBefore.nonce.toNumber();

    const amount = new anchor.BN(1_000_000);
    const receiverAddress = "CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC";

    await program.methods
      .stake(amount, receiverAddress)
      .accounts({
        senderState,
        bridgeConfig,
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

    const stateAfter = await program.account.senderState.fetch(senderState);
    assert.equal(stateAfter.nonce.toNumber(), nonceBefore + 1);
  });

  // ===== SOL-T008: Fee=0 emits full amount =====
  it("SOL-T008: stake() with fee=0 emits full amount", async () => {
    // Set fee to 0
    await program.methods
      .configureFee(new anchor.BN(0))
      .accounts({
        bridgeConfig,
        admin: admin.publicKey,
      })
      .rpc();

    const amount = new anchor.BN(1_000_000);
    const receiverAddress = "CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC";

    const listener = program.addEventListener("StakeEvent", (event: any) => {
      assert.equal(event.amount.toNumber(), 1_000_000); // full amount, no fee
    });

    await program.methods
      .stake(amount, receiverAddress)
      .accounts({
        senderState,
        bridgeConfig,
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

    await new Promise((resolve) => setTimeout(resolve, 1000));
    program.removeEventListener(listener);

    // Restore fee for other tests
    await program.methods
      .configureFee(new anchor.BN(5_000_000))
      .accounts({
        bridgeConfig,
        admin: admin.publicKey,
      })
      .rpc();
  });

  // ===== SOL-T010: Amount <= fee emits 0 =====
  it("SOL-T010: stake() with amount <= fee emits 0", async () => {
    const amount = new anchor.BN(3_000_000); // less than 5M fee

    const listener = program.addEventListener("StakeEvent", (event: any) => {
      assert.equal(event.amount.toNumber(), 0);
    });

    const receiverAddress = "CxDgkz4m1RyWCMScH9oi2rkLCM3EeAJG7UhgZoHMRxgC";

    await program.methods
      .stake(amount, receiverAddress)
      .accounts({
        senderState,
        bridgeConfig,
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

    await new Promise((resolve) => setTimeout(resolve, 1000));
    program.removeEventListener(listener);
  });
});
