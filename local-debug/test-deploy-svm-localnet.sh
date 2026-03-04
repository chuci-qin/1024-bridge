#!/usr/bin/env bash
#
# test-deploy-svm-localnet.sh
#
# Simulates a 1024chain localnet using solana-test-validator,
# deploys the SVM bridge1024 program, creates a mock USDC mint,
# and runs deploy-and-init-svm.ts to verify the full initialization flow.
#
# NOTE: The SVM program has HARDCODED_ADMIN = 2XVdXwC235qFXSm5egXpWyNY9xaiShFD5HKGrEhQNEFY
# so we must use the matching admin-svm-keypair.json to call initialize.
#
# Usage:
#   cd local-debug && bash test-deploy-svm-localnet.sh
#
# Prerequisites:
#   - solana CLI, anchor CLI, spl-token CLI installed
#   - yarn install done in both svm/bridge1024 and deploy/scripts

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SVM_DIR="$REPO_ROOT/svm/bridge1024"
DEPLOY_SCRIPTS_DIR="$REPO_ROOT/deploy/scripts"
ADMIN_KEYPAIR="$REPO_ROOT/deploy/keys/admin-svm-keypair.json"
RELAYERS_FILE="$REPO_ROOT/deploy/keys/relayers.json"
IDL_PATH="$SVM_DIR/target/idl/bridge1024.json"
RPC_URL="http://127.0.0.1:8899"

echo "============================================"
echo "  Localnet Test: SVM Bridge1024 Deploy"
echo "  (1024chain simulated via solana-test-validator)"
echo "============================================"

# ---- Step 0: Ensure admin keypair exists ----
if [ ! -f "$ADMIN_KEYPAIR" ]; then
  echo "ERROR: SVM admin keypair not found at $ADMIN_KEYPAIR"
  echo "This keypair must match HARDCODED_ADMIN in the SVM program."
  exit 1
fi
ADMIN_PUBKEY=$(solana-keygen pubkey "$ADMIN_KEYPAIR")
echo "Admin: $ADMIN_PUBKEY"
echo "(Must match HARDCODED_ADMIN: 2XVdXwC235qFXSm5egXpWyNY9xaiShFD5HKGrEhQNEFY)"

# ---- Step 1: Start solana-test-validator if not running ----
echo ""
echo ">>> Step 1: Checking local validator..."
if solana cluster-version -u "$RPC_URL" &>/dev/null; then
  echo "    Local validator already running."
  echo "    WARNING: If a previous Solana localnet is running, this will share state."
  echo "    Run 'solana-test-validator --reset' first for a clean environment."
else
  echo "    Starting solana-test-validator (simulating 1024chain)..."
  solana-test-validator --reset --quiet &
  VALIDATOR_PID=$!
  echo "    Validator PID: $VALIDATOR_PID"
  sleep 5

  if ! solana cluster-version -u "$RPC_URL" &>/dev/null; then
    echo "    Waiting for validator to start..."
    sleep 5
  fi
  echo "    Validator started."
fi

# Point Solana CLI to localnet with SVM admin keypair
solana config set --url "$RPC_URL" --keypair "$ADMIN_KEYPAIR" &>/dev/null

# ---- Step 2: Airdrop SOL to admin ----
echo ""
echo ">>> Step 2: Airdrop SOL to SVM admin..."
solana airdrop 10 "$ADMIN_PUBKEY" --url "$RPC_URL" 2>/dev/null || true
BALANCE=$(solana balance "$ADMIN_PUBKEY" --url "$RPC_URL" 2>/dev/null | awk '{print $1}')
echo "    Admin balance: $BALANCE SOL"

# ---- Step 3: Build & deploy SVM program ----
echo ""
echo ">>> Step 3: Build & deploy SVM program..."
cd "$SVM_DIR"

anchor keys sync 2>&1
anchor build 2>&1 | tail -3
anchor deploy --provider.cluster localnet --provider.wallet "$ADMIN_KEYPAIR" 2>&1 | tail -5

PROGRAM_ID=$(solana-keygen pubkey "$SVM_DIR/target/deploy/bridge1024-keypair.json")
echo "    Program ID: $PROGRAM_ID"

# ---- Step 4: Create mock USDC mint ----
echo ""
echo ">>> Step 4: Create mock USDC mint (6 decimals)..."
USDC_RESULT=$(spl-token create-token --decimals 6 --url "$RPC_URL" 2>&1)
USDC_MINT=$(echo "$USDC_RESULT" | grep "Creating token" | awk '{print $3}')

if [ -z "$USDC_MINT" ]; then
  echo "    Failed to parse mint address from: $USDC_RESULT"
  exit 1
fi
echo "    Mock USDC Mint: $USDC_MINT"

# ---- Step 5: Create admin's token account & mint some USDC ----
echo ""
echo ">>> Step 5: Create admin token account & mint mock USDC..."
spl-token create-account "$USDC_MINT" --url "$RPC_URL" 2>&1 || true
spl-token mint "$USDC_MINT" 1000000000 --url "$RPC_URL" 2>&1 || true
echo "    Minted 1,000 USDC (1000000000 smallest units) to admin"

# ---- Step 6: Install deploy script deps ----
echo ""
echo ">>> Step 6: Ensure deploy script dependencies..."
cd "$DEPLOY_SCRIPTS_DIR"
if [ ! -d "node_modules" ]; then
  npm install
fi

# ---- Step 7: Run deploy-and-init-svm.ts ----
echo ""
echo ">>> Step 7: Running deploy-and-init-svm.ts..."
echo ""

# Use a mock EVM peer contract address for localnet testing
MOCK_EVM_PEER="0x0000000000000000000000000000000000001024"

export ADMIN_KEYPAIR_PATH="$ADMIN_KEYPAIR"
export PROGRAM_ID="$PROGRAM_ID"
export SVM_RPC_URL="$RPC_URL"
export USDC_MINT="$USDC_MINT"
export PEER_CONTRACT="$MOCK_EVM_PEER"
export SOURCE_CHAIN_ID="91024"
export TARGET_CHAIN_ID="421614"
export LIQUIDITY_AMOUNT="100000000"
export SKIP_LIQUIDITY="false"
export BRIDGE_FEE="0"
export RELAYERS_FILE="$RELAYERS_FILE"
export IDL_PATH="$IDL_PATH"

npx ts-node deploy-and-init-svm.ts

echo ""
echo "============================================"
echo "  SVM Localnet test completed successfully!"
echo "============================================"
