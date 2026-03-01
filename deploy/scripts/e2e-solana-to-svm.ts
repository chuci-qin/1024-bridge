/**
 * e2e-solana-to-svm.ts
 *
 * E2E test: Solana -> 1024chain (SVM) direction.
 * Stakes USDC on Solana Bridge program, waits for sol2svm relayer to pick up
 * the event and submit signatures to 1024chain, then verifies balance increase
 * and CrossChainSuccessEvent on 1024chain.
 *
 * Environment variables:
 *   SOLANA_RPC_URL           - Solana RPC endpoint
 *   SOLANA_PROGRAM_ID        - Solana Bridge program ID
 *   SOLANA_TOKEN_ADDRESS     - Solana USDC mint address
 *   SOLANA_KEYPAIR_PATH      - Path to Solana admin keypair JSON
 *   SVM_RPC_URL              - 1024chain RPC endpoint
 *   SVM_PROGRAM_ID           - 1024chain Bridge program ID
 *   SVM_TOKEN_ADDRESS        - 1024chain USDC token address
 *   ADMIN_KEYPAIR_PATH       - Path to 1024chain admin keypair JSON
 *   IDL_PATH                 - Path to 1024chain Bridge IDL JSON
 *   TEST_AMOUNT              - Amount in e6 (default: 10000 = 0.01 USDC)
 *   BRIDGE_FEE               - Expected bridge fee in e6 (default: 0)
 *   INITIAL_DELAY_MS         - Wait before first poll (default: 5000)
 *   POLL_INTERVAL_MS         - Poll interval (default: 5000)
 *   TIMEOUT_MS               - Max wait for balance change (default: 60000)
 *   BRIDGE_ID                - Optional bridge identifier
 *
 * Status: SKELETON - will fail until Solana Bridge program + sol2svm relayer are implemented.
 */

const TAG = "solana->svm";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Missing required env: ${name}`);
  return value;
}

function log(tag: string, msg: string): void {
  console.log(`[${tag}][${new Date().toISOString()}] ${msg}`);
}

interface SolanaToSvmConfig {
  solanaRpcUrl: string;
  solanaProgramId: string;
  solanaTokenAddress: string;
  solanaKeypairPath: string;
  svmRpcUrl: string;
  svmProgramId: string;
  svmTokenAddress: string;
  adminKeypairPath: string;
  idlPath: string;
  testAmount: number;
  bridgeFee: number;
  initialDelayMs: number;
  pollIntervalMs: number;
  timeoutMs: number;
  bridgeId?: string;
}

function loadConfig(): SolanaToSvmConfig {
  return {
    solanaRpcUrl: requireEnv("SOLANA_RPC_URL"),
    solanaProgramId: requireEnv("SOLANA_PROGRAM_ID"),
    solanaTokenAddress: requireEnv("SOLANA_TOKEN_ADDRESS"),
    solanaKeypairPath: requireEnv("SOLANA_KEYPAIR_PATH"),
    svmRpcUrl: requireEnv("SVM_RPC_URL"),
    svmProgramId: requireEnv("SVM_PROGRAM_ID"),
    svmTokenAddress: requireEnv("SVM_TOKEN_ADDRESS"),
    adminKeypairPath: requireEnv("ADMIN_KEYPAIR_PATH"),
    idlPath: requireEnv("IDL_PATH"),
    testAmount: parseInt(process.env.TEST_AMOUNT || "10000"),
    bridgeFee: parseInt(process.env.BRIDGE_FEE || "0"),
    initialDelayMs: parseInt(process.env.INITIAL_DELAY_MS || "5000"),
    pollIntervalMs: parseInt(process.env.POLL_INTERVAL_MS || "5000"),
    timeoutMs: parseInt(process.env.TIMEOUT_MS || "60000"),
    bridgeId: process.env.BRIDGE_ID || undefined,
  };
}

async function main() {
  const cfg = loadConfig();

  log(TAG, "============================================");
  log(TAG, "  Bridge1024 E2E: Solana -> 1024chain");
  log(TAG, "============================================");
  log(TAG, `Solana Program: ${cfg.solanaProgramId}`);
  log(TAG, `SVM Program:    ${cfg.svmProgramId}`);
  log(TAG, `Test Amount:    ${cfg.testAmount}`);
  log(TAG, `Bridge Fee:     ${cfg.bridgeFee}`);
  log(TAG, `Expected net:   ${cfg.testAmount - cfg.bridgeFee}`);
  if (cfg.bridgeId) log(TAG, `Bridge ID:      ${cfg.bridgeId}`);
  log(TAG, "");

  // TODO: Phase 2 - Setup Solana connection + wallet
  // const solanaConnection = new Connection(cfg.solanaRpcUrl, "confirmed");
  // const solanaKeypair = loadKeypair(cfg.solanaKeypairPath);

  // TODO: Phase 2 - Setup 1024chain (SVM) connection
  // const svmConnection = new Connection(cfg.svmRpcUrl, "confirmed");
  // const svmKeypair = loadKeypair(cfg.adminKeypairPath);

  // TODO: Phase 2 - Step 1: Record 1024chain balance before
  // const svmBalBefore = await getSvmTokenBalance(svmConnection, receiverAta, tokenProgramId);
  // log(TAG, `1024chain USDC before: ${svmBalBefore}`);

  // TODO: Phase 2 - Step 2: Stake on Solana Bridge
  // log(TAG, `Staking ${cfg.testAmount} on Solana Bridge...`);
  // const stakeTx = await solanaBridge.stake(cfg.testAmount, receiver1024Address);
  // log(TAG, `Stake tx: ${stakeTx}`);

  // TODO: Phase 2 - Step 3: Verify StakeEvent on Solana
  // const stakeEvent = parseStakeEvent(stakeTx);
  // assert stakeEvent.amount === cfg.testAmount - cfg.bridgeFee

  // TODO: Phase 3 - Step 4: Wait for relayer to submit signatures to 1024chain
  // const expectedNet = cfg.testAmount - cfg.bridgeFee;
  // const svmExpected = svmBalBefore + BigInt(expectedNet);
  // const svmBalAfter = await pollUntilBalanceChanges(...);

  // TODO: Phase 4 - Step 5: Verify CrossChainSuccessEvent on 1024chain

  log(TAG, "");
  log(TAG, "SKIPPED: Solana Bridge program and sol2svm relayer not yet implemented");
  log(TAG, "This E2E test will be enabled in Phase 4 after all components are ready.");
  process.exit(0);
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(`[${TAG}] FAILED: ${err.message || err}`);
    if (err.stack) console.error(err.stack);
    process.exit(1);
  });
