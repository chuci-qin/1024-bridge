/**
 * e2e-svm-to-solana.ts
 *
 * E2E test: 1024chain (SVM) -> Solana direction.
 * Stakes USDC on 1024chain Bridge program, waits for svm2sol relayer to pick up
 * the event and submit signatures to Solana, then verifies balance increase
 * and CrossChainSuccessEvent on Solana.
 *
 * Environment variables:
 *   SOLANA_RPC_URL           - Solana RPC endpoint
 *   SOLANA_PROGRAM_ID        - Solana Bridge program ID
 *   SOLANA_TOKEN_ADDRESS     - Solana USDC mint address
 *   SOLANA_KEYPAIR_PATH      - Path to Solana keypair JSON (receiver)
 *   SVM_RPC_URL              - 1024chain RPC endpoint
 *   SVM_PROGRAM_ID           - 1024chain Bridge program ID
 *   SVM_TOKEN_ADDRESS        - 1024chain USDC token address
 *   SVM_KEYPAIR_PATH         - Path to 1024chain admin keypair JSON (sender)
 *   SOLANA_IDL_PATH          - Path to Solana Bridge IDL JSON
 *   SVM_IDL_PATH             - Path to 1024chain Bridge IDL JSON
 *   TEST_AMOUNT              - Amount in e6 (default: 10000 = 0.01 USDC)
 *   INITIAL_DELAY_MS         - Wait before first poll (default: 5000)
 *   POLL_INTERVAL_MS         - Poll interval (default: 5000)
 *   TIMEOUT_MS               - Max wait for balance change (default: 60000)
 *   BRIDGE_ID                - Optional bridge identifier
 *
 * Status: SKELETON - will fail until svm2sol relayer is deployed.
 */

const TAG = "svm->solana";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Missing required env: ${name}`);
  return value;
}

function log(tag: string, msg: string): void {
  console.log(`[${tag}][${new Date().toISOString()}] ${msg}`);
}

interface SvmToSolanaConfig {
  solanaRpcUrl: string;
  solanaProgramId: string;
  solanaTokenAddress: string;
  solanaKeypairPath: string;
  svmRpcUrl: string;
  svmProgramId: string;
  svmTokenAddress: string;
  svmKeypairPath: string;
  solanaIdlPath: string;
  svmIdlPath: string;
  testAmount: number;
  initialDelayMs: number;
  pollIntervalMs: number;
  timeoutMs: number;
  bridgeId?: string;
}

function loadConfig(): SvmToSolanaConfig {
  return {
    solanaRpcUrl: requireEnv("SOLANA_RPC_URL"),
    solanaProgramId: requireEnv("SOLANA_PROGRAM_ID"),
    solanaTokenAddress: requireEnv("SOLANA_TOKEN_ADDRESS"),
    solanaKeypairPath: requireEnv("SOLANA_KEYPAIR_PATH"),
    svmRpcUrl: requireEnv("SVM_RPC_URL"),
    svmProgramId: requireEnv("SVM_PROGRAM_ID"),
    svmTokenAddress: requireEnv("SVM_TOKEN_ADDRESS"),
    svmKeypairPath: requireEnv("SVM_KEYPAIR_PATH"),
    solanaIdlPath: requireEnv("SOLANA_IDL_PATH"),
    svmIdlPath: requireEnv("SVM_IDL_PATH"),
    testAmount: parseInt(process.env.TEST_AMOUNT || "10000"),
    initialDelayMs: parseInt(process.env.INITIAL_DELAY_MS || "5000"),
    pollIntervalMs: parseInt(process.env.POLL_INTERVAL_MS || "5000"),
    timeoutMs: parseInt(process.env.TIMEOUT_MS || "60000"),
    bridgeId: process.env.BRIDGE_ID || undefined,
  };
}

async function main() {
  const cfg = loadConfig();

  log(TAG, "============================================");
  log(TAG, "  Bridge1024 E2E: 1024chain -> Solana");
  log(TAG, "============================================");
  log(TAG, `SVM Program:    ${cfg.svmProgramId}`);
  log(TAG, `Solana Program: ${cfg.solanaProgramId}`);
  log(TAG, `Test Amount:    ${cfg.testAmount}`);
  if (cfg.bridgeId) log(TAG, `Bridge ID:      ${cfg.bridgeId}`);
  log(TAG, "");

  // TODO: Step 1 - Setup 1024chain connection + wallet
  // const svmConnection = new Connection(cfg.svmRpcUrl, "confirmed");
  // const svmKeypair = loadKeypair(cfg.svmKeypairPath);

  // TODO: Step 2 - Setup Solana connection
  // const solanaConnection = new Connection(cfg.solanaRpcUrl, "confirmed");
  // const solanaKeypair = loadKeypair(cfg.solanaKeypairPath);

  // TODO: Step 3 - Record Solana USDC balance before
  // const solanaBalBefore = await getSolanaTokenBalance(solanaConnection, receiverAta);
  // log(TAG, `Solana USDC before: ${solanaBalBefore}`);

  // TODO: Step 4 - Stake on 1024chain Bridge
  // log(TAG, `Staking ${cfg.testAmount} on 1024chain Bridge...`);
  // const stakeTx = await svmBridge.stake(cfg.testAmount, receiverSolanaAddress);
  // log(TAG, `Stake tx: ${stakeTx}`);

  // TODO: Step 5 - Wait for svm2sol relayer to submit signatures to Solana
  // Poll Solana receiver balance until it increases by expected amount
  // Note: Solana charges NO fee, so expected = full event_data.amount

  // TODO: Step 6 - Verify CrossChainSuccessEvent on Solana

  log(TAG, "");
  log(TAG, "SKIPPED: svm2sol relayer not yet deployed");
  log(TAG, "This E2E test will be enabled after deployment.");
  process.exit(0);
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(`[${TAG}] FAILED: ${err.message || err}`);
    if (err.stack) console.error(err.stack);
    process.exit(1);
  });
