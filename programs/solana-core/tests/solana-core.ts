import * as anchor from "@coral-xyz/anchor";
import { Program, Idl } from "@coral-xyz/anchor";
import { expect } from "chai";
import { Keypair, PublicKey, SystemProgram, LAMPORTS_PER_SOL } from "@solana/web3.js";
import * as fs from "fs";
import * as path from "path";

describe("solana-core", () => {
  // Configure the client to use the local cluster.
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  // Load IDL directly from file to avoid workspace issues
  const idlPath = path.join(__dirname, "../target/idl/solana_core.json");
  const idlJson = JSON.parse(fs.readFileSync(idlPath, "utf8"));
  
  // Program ID from IDL
  const programId = new PublicKey(idlJson.address);
  const program = new Program(idlJson as Idl, programId, provider);

  // Test accounts
  let bridgePda: PublicKey;
  let bridgeBump: number;
  let guardianSetPda: PublicKey;
  let guardianSetBump: number;
  
  // Guardian keys (Ethereum address format - 20 bytes)
  const initialGuardians = [
    Array.from(Buffer.from("1111111111111111111111111111111111111111", "hex")),
    Array.from(Buffer.from("2222222222222222222222222222222222222222", "hex")),
    Array.from(Buffer.from("3333333333333333333333333333333333333333", "hex")),
  ];

  const guardianSetIndex = 0;

  before(async () => {
    // Derive PDAs
    [bridgePda, bridgeBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("bridge")],
      program.programId
    );

    [guardianSetPda, guardianSetBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("guardian_set"), Buffer.from([guardianSetIndex, 0, 0, 0])],
      program.programId
    );
  });

  describe("initialize", () => {
    it("Initializes the bridge with guardian set", async () => {
      const tx = await program.methods
        .initialize(guardianSetIndex, initialGuardians)
        .accounts({
          bridge: bridgePda,
          guardianSet: guardianSetPda,
          payer: provider.wallet.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      console.log("Initialize transaction signature:", tx);

      // Verify bridge account
      const bridgeAccount = await program.account.bridge.fetch(bridgePda);
      expect(bridgeAccount.guardianSetIndex).to.equal(guardianSetIndex);
      expect(bridgeAccount.config.chainId).to.equal(2); // Solana chain ID
      expect(bridgeAccount.config.feeLamports.toNumber()).to.equal(1_000_000);

      // Verify guardian set
      const guardianSetAccount = await program.account.guardianSet.fetch(guardianSetPda);
      expect(guardianSetAccount.index).to.equal(guardianSetIndex);
      expect(guardianSetAccount.keys.length).to.equal(3);
      expect(guardianSetAccount.expirationTime).to.equal(0); // Active
      
      // Verify guardian keys
      for (let i = 0; i < initialGuardians.length; i++) {
        const expected = initialGuardians[i];
        const actual = Array.from(guardianSetAccount.keys[i]);
        expect(actual).to.deep.equal(expected);
      }
    });

    it("Fails to initialize with no guardians", async () => {
      const emptyGuardians = [];
      const newIndex = 1;
      
      const [newGuardianSetPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("guardian_set"), Buffer.from([newIndex, 0, 0, 0])],
        program.programId
      );

      try {
        await program.methods
          .initialize(newIndex, emptyGuardians)
          .accounts({
            bridge: bridgePda,
            guardianSet: newGuardianSetPda,
            payer: provider.wallet.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
        
        expect.fail("Should have failed with no guardians");
      } catch (error) {
        expect(error.toString()).to.include("NoGuardiansProvided");
      }
    });
  });

  describe("post_message", () => {
    let emitter: Keypair;
    let sequencePda: PublicKey;
    let messagePda: PublicKey;
    let payer: Keypair;

    before(async () => {
      emitter = Keypair.generate();
      payer = Keypair.generate();
      
      // Airdrop SOL to accounts
      const airdropSig1 = await provider.connection.requestAirdrop(
        emitter.publicKey,
        2 * LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(airdropSig1);
      
      const airdropSig2 = await provider.connection.requestAirdrop(
        payer.publicKey,
        2 * LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(airdropSig2);
    });

    it("Posts a message and increments sequence", async () => {
      const sequence = 0; // First message
      const nonce = 12345;
      const payload = Buffer.from("Hello from Solana!");
      const consistencyLevel = 200; // Finalized

      // Derive sequence PDA
      [sequencePda] = PublicKey.findProgramAddressSync(
        [Buffer.from("sequence"), emitter.publicKey.toBuffer()],
        program.programId
      );

      // Derive message PDA
      const sequenceBytes = Buffer.alloc(8);
      sequenceBytes.writeBigUInt64LE(BigInt(sequence));
      
      [messagePda] = PublicKey.findProgramAddressSync(
        [
          Buffer.from("message"),
          emitter.publicKey.toBuffer(),
          sequenceBytes,
        ],
        program.programId
      );

      const tx = await program.methods
        .postMessage(new anchor.BN(sequence), nonce, Array.from(payload), consistencyLevel)
        .accounts({
          bridge: bridgePda,
          message: messagePda,
          emitter: emitter.publicKey,
          sequenceAccount: sequencePda,
          payer: payer.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([emitter, payer])
        .rpc();

      console.log("Post message transaction signature:", tx);

      // Verify message account
      const messageAccount = await program.account.postedMessage.fetch(messagePda);
      expect(messageAccount.consistencyLevel).to.equal(consistencyLevel);
      expect(messageAccount.emitterChain).to.equal(2); // Solana
      expect(messageAccount.emitterAddress).to.deep.equal(
        Array.from(emitter.publicKey.toBytes())
      );
      expect(messageAccount.sequence.toNumber()).to.equal(0);
      expect(messageAccount.nonce).to.equal(nonce);
      expect(Buffer.from(messageAccount.payload)).to.deep.equal(payload);

      // Verify sequence account
      const sequenceAccount = await program.account.sequence.fetch(sequencePda);
      expect(sequenceAccount.value.toNumber()).to.equal(1);
    });

    it("Posts multiple messages with incrementing sequence", async () => {
      const sequence = 1; // Second message
      const nonce = 54321;
      const payload = Buffer.from("Second message");
      const consistencyLevel = 200;

      // Derive message PDA for sequence 1
      const sequenceBytes = Buffer.alloc(8);
      sequenceBytes.writeBigUInt64LE(BigInt(sequence));
      
      const [message2Pda] = PublicKey.findProgramAddressSync(
        [
          Buffer.from("message"),
          emitter.publicKey.toBuffer(),
          sequenceBytes,
        ],
        program.programId
      );

      await program.methods
        .postMessage(new anchor.BN(sequence), nonce, Array.from(payload), consistencyLevel)
        .accounts({
          bridge: bridgePda,
          message: message2Pda,
          emitter: emitter.publicKey,
          sequenceAccount: sequencePda,
          payer: payer.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([emitter, payer])
        .rpc();

      // Verify sequence incremented
      const sequenceAccount = await program.account.sequence.fetch(sequencePda);
      expect(sequenceAccount.value.toNumber()).to.equal(2);

      // Verify second message
      const message2Account = await program.account.postedMessage.fetch(message2Pda);
      expect(message2Account.sequence.toNumber()).to.equal(1);
    });
  });

  describe("post_vaa", () => {
    let postedVaaPda: PublicKey;
    let payer: Keypair;

    before(async () => {
      payer = Keypair.generate();
      
      const airdropSig = await provider.connection.requestAirdrop(
        payer.publicKey,
        2 * LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(airdropSig);
    });

    it("Posts a valid VAA", async () => {
      const vaaVersion = 1;
      const vaaGuardianSet = guardianSetIndex;
      const vaaSignaturesLen = 3; // All 3 guardians sign (>= 2/3 + 1 = 3)
      const vaaTimestamp = Math.floor(Date.now() / 1000);
      const vaaNonce = 999;
      const vaaEmitterChain = 1; // Ethereum
      const vaaEmitterAddress = Array(32).fill(0x11);
      const vaaSequence = BigInt(42);
      const vaaConsistencyLevel = 200;
      const vaaPayload = Array.from(Buffer.from("Cross-chain payload"));

      // Generate a unique keypair for the posted VAA account
      const postedVaaKeypair = Keypair.generate();

      const tx = await program.methods
        .postVaa(
          vaaVersion,
          vaaGuardianSet,
          vaaSignaturesLen,
          vaaTimestamp,
          vaaNonce,
          vaaEmitterChain,
          vaaEmitterAddress,
          new anchor.BN(vaaSequence.toString()),
          vaaConsistencyLevel,
          vaaPayload
        )
        .accounts({
          bridge: bridgePda,
          guardianSet: guardianSetPda,
          postedVaa: postedVaaKeypair.publicKey,
          payer: payer.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([payer, postedVaaKeypair])
        .rpc();

      console.log("Post VAA transaction signature:", tx);

      // Verify posted VAA account
      const postedVaaAccount = await program.account.postedVaa.fetch(
        postedVaaKeypair.publicKey
      );
      
      expect(postedVaaAccount.guardianSetIndex).to.equal(vaaGuardianSet);
      expect(postedVaaAccount.emitterChain).to.equal(vaaEmitterChain);
      expect(postedVaaAccount.emitterAddress).to.deep.equal(vaaEmitterAddress);
      expect(postedVaaAccount.sequence.toNumber()).to.equal(Number(vaaSequence));
      expect(postedVaaAccount.timestamp).to.equal(vaaTimestamp);
      expect(postedVaaAccount.nonce).to.equal(vaaNonce);
      expect(postedVaaAccount.consistencyLevel).to.equal(vaaConsistencyLevel);
      expect(postedVaaAccount.payload).to.deep.equal(vaaPayload);
    });

    it("Fails with invalid VAA version", async () => {
      const invalidVersion = 2; // Only version 1 is supported
      const vaaGuardianSet = guardianSetIndex;
      const vaaSignaturesLen = 3;
      const vaaTimestamp = Math.floor(Date.now() / 1000);
      const vaaNonce = 1000;
      const vaaEmitterChain = 1;
      const vaaEmitterAddress = Array(32).fill(0x22);
      const vaaSequence = BigInt(43);
      const vaaConsistencyLevel = 200;
      const vaaPayload = Array.from(Buffer.from("Invalid version"));

      const postedVaaKeypair = Keypair.generate();

      try {
        await program.methods
          .postVaa(
            invalidVersion,
            vaaGuardianSet,
            vaaSignaturesLen,
            vaaTimestamp,
            vaaNonce,
            vaaEmitterChain,
            vaaEmitterAddress,
            new anchor.BN(vaaSequence.toString()),
            vaaConsistencyLevel,
            vaaPayload
          )
          .accounts({
            bridge: bridgePda,
            guardianSet: guardianSetPda,
            postedVaa: postedVaaKeypair.publicKey,
            payer: payer.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .signers([payer, postedVaaKeypair])
          .rpc();

        expect.fail("Should have failed with invalid VAA version");
      } catch (error) {
        expect(error.toString()).to.include("InvalidVAAVersion");
      }
    });

    it("Fails with mismatched guardian set", async () => {
      const vaaVersion = 1;
      const wrongGuardianSet = 99; // Non-existent guardian set
      const vaaSignaturesLen = 3;
      const vaaTimestamp = Math.floor(Date.now() / 1000);
      const vaaNonce = 1001;
      const vaaEmitterChain = 1;
      const vaaEmitterAddress = Array(32).fill(0x33);
      const vaaSequence = BigInt(44);
      const vaaConsistencyLevel = 200;
      const vaaPayload = Array.from(Buffer.from("Wrong guardian set"));

      const postedVaaKeypair = Keypair.generate();

      try {
        await program.methods
          .postVaa(
            vaaVersion,
            wrongGuardianSet,
            vaaSignaturesLen,
            vaaTimestamp,
            vaaNonce,
            vaaEmitterChain,
            vaaEmitterAddress,
            new anchor.BN(vaaSequence.toString()),
            vaaConsistencyLevel,
            vaaPayload
          )
          .accounts({
            bridge: bridgePda,
            guardianSet: guardianSetPda,
            postedVaa: postedVaaKeypair.publicKey,
            payer: payer.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .signers([payer, postedVaaKeypair])
          .rpc();

        expect.fail("Should have failed with invalid guardian set");
      } catch (error) {
        expect(error.toString()).to.include("InvalidGuardianSet");
      }
    });

    it("Fails with insufficient signatures", async () => {
      const vaaVersion = 1;
      const vaaGuardianSet = guardianSetIndex;
      const vaaSignaturesLen = 1; // Only 1 signature, need at least 3 (2/3 + 1)
      const vaaTimestamp = Math.floor(Date.now() / 1000);
      const vaaNonce = 1002;
      const vaaEmitterChain = 1;
      const vaaEmitterAddress = Array(32).fill(0x44);
      const vaaSequence = BigInt(45);
      const vaaConsistencyLevel = 200;
      const vaaPayload = Array.from(Buffer.from("Not enough sigs"));

      const postedVaaKeypair = Keypair.generate();

      try {
        await program.methods
          .postVaa(
            vaaVersion,
            vaaGuardianSet,
            vaaSignaturesLen,
            vaaTimestamp,
            vaaNonce,
            vaaEmitterChain,
            vaaEmitterAddress,
            new anchor.BN(vaaSequence.toString()),
            vaaConsistencyLevel,
            vaaPayload
          )
          .accounts({
            bridge: bridgePda,
            guardianSet: guardianSetPda,
            postedVaa: postedVaaKeypair.publicKey,
            payer: payer.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .signers([payer, postedVaaKeypair])
          .rpc();

        expect.fail("Should have failed with insufficient signatures");
      } catch (error) {
        expect(error.toString()).to.include("InsufficientSignatures");
      }
    });
  });

  describe("verify_vaa_signatures", () => {
    it("Verifies signatures with sufficient quorum", async () => {
      const hash = Array(32).fill(0xaa);
      const signaturesCount = 3; // All 3 guardians

      const tx = await program.methods
        .verifyVaaSignatures(hash, signaturesCount)
        .accounts({
          guardianSet: guardianSetPda,
        })
        .rpc();

      console.log("Verify signatures transaction signature:", tx);
    });

    it("Fails with insufficient signatures", async () => {
      const hash = Array(32).fill(0xbb);
      const signaturesCount = 1; // Need at least 3 (2/3 + 1)

      try {
        await program.methods
          .verifyVaaSignatures(hash, signaturesCount)
          .accounts({
            guardianSet: guardianSetPda,
          })
          .rpc();

        expect.fail("Should have failed with insufficient signatures");
      } catch (error) {
        expect(error.toString()).to.include("InsufficientSignatures");
      }
    });
  });

  describe("Integration: Full message flow", () => {
    it("Posts message on Solana, simulates VAA, and verifies", async () => {
      // 1. Create new emitter
      const emitter = Keypair.generate();
      const payer = Keypair.generate();
      
      const airdropSig1 = await provider.connection.requestAirdrop(
        emitter.publicKey,
        2 * LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(airdropSig1);
      
      const airdropSig2 = await provider.connection.requestAirdrop(
        payer.publicKey,
        2 * LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(airdropSig2);

      // 2. Post message
      const sequence = 0; // First message from this emitter
      const nonce = 99999;
      const payload = Buffer.from("Integration test payload");
      const consistencyLevel = 200;

      const [sequencePda] = PublicKey.findProgramAddressSync(
        [Buffer.from("sequence"), emitter.publicKey.toBuffer()],
        program.programId
      );

      const sequenceBytes = Buffer.alloc(8);
      sequenceBytes.writeBigUInt64LE(BigInt(sequence));
      
      const [messagePda] = PublicKey.findProgramAddressSync(
        [
          Buffer.from("message"),
          emitter.publicKey.toBuffer(),
          sequenceBytes,
        ],
        program.programId
      );

      await program.methods
        .postMessage(new anchor.BN(sequence), nonce, Array.from(payload), consistencyLevel)
        .accounts({
          bridge: bridgePda,
          message: messagePda,
          emitter: emitter.publicKey,
          sequenceAccount: sequencePda,
          payer: payer.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([emitter, payer])
        .rpc();

      const messageAccount = await program.account.postedMessage.fetch(messagePda);
      
      // 3. Simulate guardian observing and creating VAA
      const vaaVersion = 1;
      const vaaGuardianSet = guardianSetIndex;
      const vaaSignaturesLen = 3;
      const vaaEmitterChain = messageAccount.emitterChain;
      const vaaEmitterAddress = messageAccount.emitterAddress;
      const vaaSequence = messageAccount.sequence;
      const vaaTimestamp = messageAccount.timestamp;
      const vaaNonce = messageAccount.nonce;
      const vaaConsistencyLevel = messageAccount.consistencyLevel;
      const vaaPayload = messageAccount.payload;

      // 4. Post VAA (simulating relayer)
      const postedVaaKeypair = Keypair.generate();

      await program.methods
        .postVaa(
          vaaVersion,
          vaaGuardianSet,
          vaaSignaturesLen,
          vaaTimestamp,
          vaaNonce,
          vaaEmitterChain,
          vaaEmitterAddress,
          vaaSequence,
          vaaConsistencyLevel,
          vaaPayload
        )
        .accounts({
          bridge: bridgePda,
          guardianSet: guardianSetPda,
          postedVaa: postedVaaKeypair.publicKey,
          payer: payer.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([payer, postedVaaKeypair])
        .rpc();

      const postedVaaAccount = await program.account.postedVaa.fetch(
        postedVaaKeypair.publicKey
      );

      // 5. Verify the posted VAA matches the original message
      expect(postedVaaAccount.emitterChain).to.equal(messageAccount.emitterChain);
      expect(postedVaaAccount.emitterAddress).to.deep.equal(messageAccount.emitterAddress);
      expect(postedVaaAccount.sequence.toString()).to.equal(messageAccount.sequence.toString());
      expect(postedVaaAccount.nonce).to.equal(messageAccount.nonce);
      expect(postedVaaAccount.payload).to.deep.equal(messageAccount.payload);
      
      console.log("✓ Full integration test passed");
    });
  });
});

