#!/usr/bin/env bash
#
# test-deploy-solana-localnet.sh
#
# Spins up a local Solana validator, deploys the bridge1024_solana program,
# creates a mock USDC mint, and runs deploy-and-init-solana.ts to verify
# the full initialization flow on localnet.
#
# Usage:
#   cd local-debug && bash test-deploy-solana-localnet.sh
#
# Prerequisites:
#   - solana CLI, anchor CLI, spl-token CLI installed
#   - yarn install done in both solana/bridge1024 and deploy/scripts

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SOLANA_DIR="$REPO_ROOT/solana/bridge1024"
DEPLOY_SCRIPTS_DIR="$REPO_ROOT/deploy/scripts"
ADMIN_KEYPAIR="$REPO_ROOT/deploy/keys/admin-solana-keypair.json"
RELAYERS_FILE="$REPO_ROOT/deploy/keys/relayers.json"
IDL_PATH="$SOLANA_DIR/target/idl/bridge1024_solana.json"
RPC_URL="http://127.0.0.1:8899"

echo "============================================"
echo "  Localnet Test: Solana Bridge1024 Deploy"
echo "============================================"

# ---- Step 0: Ensure admin keypair exists ----
if [ ! -f "$ADMIN_KEYPAIR" ]; then
  echo ">>> Creating admin keypair..."
  solana-keygen new --no-bip39-passphrase -o "$ADMIN_KEYPAIR"
fi
ADMIN_PUBKEY=$(solana-keygen pubkey "$ADMIN_KEYPAIR")
echo "Admin: $ADMIN_PUBKEY"

# ---- Step 1: Start solana-test-validator if not running ----
echo ""
echo ">>> Step 1: Checking local validator..."
if solana cluster-version -u "$RPC_URL" &>/dev/null; then
  echo "    Local validator already running."
else
  echo "    Starting solana-test-validator..."
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

# Point Solana CLI to localnet
solana config set --url "$RPC_URL" --keypair "$ADMIN_KEYPAIR" &>/dev/null

# ---- Step 2: Airdrop SOL to admin ----
echo ""
echo ">>> Step 2: Airdrop SOL to admin..."
solana airdrop 10 "$ADMIN_PUBKEY" --url "$RPC_URL" 2>/dev/null || true
BALANCE=$(solana balance "$ADMIN_PUBKEY" --url "$RPC_URL" 2>/dev/null | awk '{print $1}')
echo "    Admin balance: $BALANCE SOL"

# ---- Step 3: Build & deploy program ----
echo ""
echo ">>> Step 3: Build & deploy program..."
cd "$SOLANA_DIR"

# Ensure Anchor.toml points to localnet
if grep -q 'cluster = "devnet"' Anchor.toml; then
  echo "    Switching Anchor.toml to localnet..."
  sed -i 's/cluster = "devnet"/cluster = "localnet"/' Anchor.toml
fi

anchor keys sync 2>&1
anchor build 2>&1 | tail -3
anchor deploy --provider.cluster localnet 2>&1 | tail -5

PROGRAM_ID=$(solana-keygen pubkey "$SOLANA_DIR/target/deploy/bridge1024_solana-keypair.json")
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

# ---- Step 7: Run deploy-and-init-solana.ts ----
echo ""
echo ">>> Step 7: Running deploy-and-init-solana.ts..."
echo ""

export ADMIN_KEYPAIR_PATH="$ADMIN_KEYPAIR"
export PROGRAM_ID="$PROGRAM_ID"
export SOLANA_RPC_URL="$RPC_URL"
export USDC_MINT="$USDC_MINT"
export PEER_CONTRACT="11111111111111111111111111111111"
export SOURCE_CHAIN_ID="103"
export TARGET_CHAIN_ID="91024"
export LIQUIDITY_AMOUNT="100000000"
export SKIP_LIQUIDITY="false"
export RELAYERS_FILE="$RELAYERS_FILE"
export IDL_PATH="$IDL_PATH"

npx ts-node deploy-and-init-solana.ts

echo ""
echo "============================================"
echo "  Localnet test completed successfully!"
echo "============================================"
