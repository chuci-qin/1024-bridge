#!/usr/bin/env bash
# deploy-all.sh
# One-shot: build + deploy + init + register relayers on Arbitrum Sepolia + 1024chain Testnet
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

BRIDGE_ID="arbsep-1024test-usdc"
BRIDGES_FILE="$PROJECT_ROOT/deploy/config/bridges.json"
ADMIN_EVM_KEY_FILE="$PROJECT_ROOT/deploy/keys/admin-evm-private-key.json"
ADMIN_SVM_KEYPAIR="$PROJECT_ROOT/deploy/keys/admin-svm-keypair.json"
RELAYERS_FILE="$PROJECT_ROOT/deploy/keys/relayers.json"
DEPLOYMENT_FILE="$SCRIPT_DIR/deployment.json"

log() { echo "[deploy][$(date '+%H:%M:%S')] $*"; }
die() { echo "[deploy] ERROR: $*" >&2; exit 1; }

# ---- Parse bridge config ----
BRIDGE_CFG=$(jq -r ".\"$BRIDGE_ID\"" "$BRIDGES_FILE")
EVM_RPC=$(echo "$BRIDGE_CFG" | jq -r '.evm.rpc_url')
EVM_CHAIN_ID=$(echo "$BRIDGE_CFG" | jq -r '.evm.chain_id')
EVM_TOKEN=$(echo "$BRIDGE_CFG" | jq -r '.evm.token_address')
EVM_CONFIRMS=$(echo "$BRIDGE_CFG" | jq -r '.evm.confirmation_blocks')
SVM_RPC=$(echo "$BRIDGE_CFG" | jq -r '.svm.rpc_url')
SVM_CHAIN_ID=$(echo "$BRIDGE_CFG" | jq -r '.svm.chain_id')
SVM_TOKEN=$(echo "$BRIDGE_CFG" | jq -r '.svm.token_address')
DECIMAL_RATIO=$(echo "$BRIDGE_CFG" | jq -r '.decimal_ratio // 1')
LIQUIDITY=$(echo "$BRIDGE_CFG" | jq -r '.liquidity_amount // "1000000"')

ADMIN_EVM_KEY=$(jq -r '.private_key' "$ADMIN_EVM_KEY_FILE")
ADMIN_EVM_ADDR=$(jq -r '.address' "$ADMIN_EVM_KEY_FILE")

log "=========================================="
log "  Local Debug - Deploy All"
log "=========================================="
log "Bridge:       $BRIDGE_ID"
log "EVM RPC:      $EVM_RPC"
log "EVM Chain ID: $EVM_CHAIN_ID"
log "SVM RPC:      $SVM_RPC"
log "SVM Chain ID: $SVM_CHAIN_ID"
log "Admin EVM:    $ADMIN_EVM_ADDR"
log ""

# ============================================================
# Step 1: Build EVM contracts
# ============================================================
log ">>> Step 1: Building EVM contracts..."
cd "$PROJECT_ROOT/evm/bridge1024"
forge build --sizes 2>&1 | tail -5
log "EVM build complete."

# ============================================================
# Step 2: Build SVM contracts (fresh program keypair)
# ============================================================
log ">>> Step 2: Building SVM contracts..."
cd "$PROJECT_ROOT/svm/bridge1024"

PROGRAM_KEYPAIR="/tmp/bridge1024-program-keypair.json"
solana-keygen new -o "$PROGRAM_KEYPAIR" --no-bip39-passphrase --force 2>/dev/null
SVM_PROGRAM_ID=$(solana-keygen pubkey "$PROGRAM_KEYPAIR")
log "Generated new SVM Program ID: $SVM_PROGRAM_ID"

sed -i "s/^bridge1024 = \".*\"/bridge1024 = \"$SVM_PROGRAM_ID\"/" Anchor.toml
sed -i "s/declare_id!(\".*\")/declare_id!(\"$SVM_PROGRAM_ID\")/" programs/bridge1024/src/lib.rs

anchor build 2>&1 | tail -3
log "SVM build complete."

# ============================================================
# Step 3: Deploy EVM contract
# ============================================================
log ">>> Step 3: Deploying EVM contract to Arbitrum Sepolia..."
cd "$PROJECT_ROOT/evm/bridge1024"

DEPLOY_OUTPUT=$(forge create \
  --rpc-url "$EVM_RPC" \
  --private-key "$ADMIN_EVM_KEY" \
  --broadcast \
  src/Bridge1024.sol:Bridge1024 \
  --constructor-args "$ADMIN_EVM_ADDR" 2>&1) || die "EVM deploy failed: $DEPLOY_OUTPUT"

EVM_CONTRACT=$(echo "$DEPLOY_OUTPUT" | grep -oP 'Deployed to: \K0x[0-9a-fA-F]+')
[ -n "$EVM_CONTRACT" ] || die "Failed to extract EVM contract address from:\n$DEPLOY_OUTPUT"
log "EVM contract deployed: $EVM_CONTRACT"

# ============================================================
# Step 4: Deploy SVM program
# ============================================================
log ">>> Step 4: Deploying SVM program to 1024chain..."
solana config set --url "$SVM_RPC" --keypair "$ADMIN_SVM_KEYPAIR" 2>/dev/null

solana program deploy \
  --url "$SVM_RPC" \
  --keypair "$ADMIN_SVM_KEYPAIR" \
  --program-id "$PROGRAM_KEYPAIR" \
  "$PROJECT_ROOT/svm/bridge1024/target/deploy/bridge1024.so" 2>&1 | tail -3

log "SVM program deployed: $SVM_PROGRAM_ID"

# ============================================================
# Step 5: Initialize EVM contract
# ============================================================
log ">>> Step 5: Initializing EVM contract..."
PRIVATE_KEY="$ADMIN_EVM_KEY" \
RPC_URL="$EVM_RPC" \
CONTRACT_ADDRESS="$EVM_CONTRACT" \
TOKEN_ADDRESS="$EVM_TOKEN" \
PEER_CONTRACT="$SVM_PROGRAM_ID" \
SOURCE_CHAIN_ID="$EVM_CHAIN_ID" \
TARGET_CHAIN_ID="$SVM_CHAIN_ID" \
DECIMAL_RATIO="$DECIMAL_RATIO" \
LIQUIDITY_AMOUNT="$LIQUIDITY" \
SKIP_LIQUIDITY="true" \
RELAYERS_FILE="$RELAYERS_FILE" \
  bash "$PROJECT_ROOT/deploy/scripts/deploy-and-init-evm.sh"

log "EVM contract initialized."

# ============================================================
# Step 6: Initialize SVM program
# ============================================================
log ">>> Step 6: Initializing SVM program..."
cd "$PROJECT_ROOT/deploy/scripts"

ADMIN_KEYPAIR_PATH="$ADMIN_SVM_KEYPAIR" \
PROGRAM_ID="$SVM_PROGRAM_ID" \
SVM_RPC_URL="$SVM_RPC" \
USDC_MINT="$SVM_TOKEN" \
PEER_CONTRACT="$EVM_CONTRACT" \
SOURCE_CHAIN_ID="$SVM_CHAIN_ID" \
TARGET_CHAIN_ID="$EVM_CHAIN_ID" \
LIQUIDITY_AMOUNT="$LIQUIDITY" \
SKIP_LIQUIDITY="true" \
RELAYERS_FILE="$RELAYERS_FILE" \
IDL_PATH="$PROJECT_ROOT/svm/bridge1024/target/idl/bridge1024.json" \
  npx ts-node deploy-and-init-svm.ts

log "SVM program initialized."

# ============================================================
# Step 7: Save deployment result
# ============================================================
jq -n \
  --arg evm_contract "$EVM_CONTRACT" \
  --arg svm_program "$SVM_PROGRAM_ID" \
  --arg evm_rpc "$EVM_RPC" \
  --arg svm_rpc "$SVM_RPC" \
  --arg evm_chain_id "$EVM_CHAIN_ID" \
  --arg svm_chain_id "$SVM_CHAIN_ID" \
  --arg evm_token "$EVM_TOKEN" \
  --arg svm_token "$SVM_TOKEN" \
  --arg bridge_id "$BRIDGE_ID" \
  --arg deployed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{
    bridge_id: $bridge_id,
    deployed_at: $deployed_at,
    evm_contract_address: $evm_contract,
    svm_program_id: $svm_program,
    evm_rpc_url: $evm_rpc,
    svm_rpc_url: $svm_rpc,
    evm_chain_id: ($evm_chain_id | tonumber),
    svm_chain_id: ($svm_chain_id | tonumber),
    evm_token_address: $evm_token,
    svm_token_address: $svm_token
  }' > "$DEPLOYMENT_FILE"

log ""
log "=========================================="
log "  Deployment Complete!"
log "=========================================="
log "EVM Contract:  $EVM_CONTRACT"
log "SVM Program:   $SVM_PROGRAM_ID"
log "Saved to:      $DEPLOYMENT_FILE"
log ""

rm -f "$PROGRAM_KEYPAIR"
