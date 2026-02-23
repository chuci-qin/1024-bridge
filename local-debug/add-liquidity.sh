#!/usr/bin/env bash
# add-liquidity.sh — Add USDC liquidity to both EVM and SVM contracts
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOYMENT_FILE="$SCRIPT_DIR/deployment.json"

AMOUNT="${1:-1000000}"  # default 1 USDC (6 decimals)

log() { echo "[liquidity][$(date '+%H:%M:%S')] $*"; }

[ -f "$DEPLOYMENT_FILE" ] || { echo "ERROR: deployment.json not found"; exit 1; }

EVM_CONTRACT=$(jq -r '.evm_contract_address' "$DEPLOYMENT_FILE")
SVM_PROGRAM=$(jq -r '.svm_program_id' "$DEPLOYMENT_FILE")
EVM_RPC=$(jq -r '.evm_rpc_url' "$DEPLOYMENT_FILE")
SVM_RPC=$(jq -r '.svm_rpc_url' "$DEPLOYMENT_FILE")
EVM_TOKEN=$(jq -r '.evm_token_address' "$DEPLOYMENT_FILE")
SVM_TOKEN=$(jq -r '.svm_token_address' "$DEPLOYMENT_FILE")

ADMIN_EVM_KEY=$(jq -r '.private_key' "$PROJECT_ROOT/deploy/keys/admin-evm-private-key.json")
ADMIN_SVM_KEYPAIR="$PROJECT_ROOT/deploy/keys/admin-svm-keypair.json"

log "Adding $AMOUNT tokens liquidity to both sides..."

# ---- EVM: transfer USDC to contract (contract is vault in v2.0) ----
log ">>> EVM: Transferring $AMOUNT USDC to $EVM_CONTRACT..."
cast send "$EVM_TOKEN" \
  "transfer(address,uint256)" \
  "$EVM_CONTRACT" \
  "$AMOUNT" \
  --rpc-url "$EVM_RPC" \
  --private-key "$ADMIN_EVM_KEY" 2>&1 | tail -3

EVM_BAL=$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$EVM_CONTRACT" --rpc-url "$EVM_RPC")
log "EVM contract USDC balance: $EVM_BAL"

# ---- SVM: add liquidity via program instruction ----
log ">>> SVM: Adding $AMOUNT USDC to vault..."
cd "$PROJECT_ROOT/deploy/scripts"

ADMIN_KEYPAIR_PATH="$ADMIN_SVM_KEYPAIR" \
PROGRAM_ID="$SVM_PROGRAM" \
SVM_RPC_URL="$SVM_RPC" \
USDC_MINT="$SVM_TOKEN" \
LIQUIDITY_AMOUNT="$AMOUNT" \
IDL_PATH="$PROJECT_ROOT/svm/bridge1024/target/idl/bridge1024.json" \
  npx ts-node add-liquidity-svm.ts

log ""
log "Liquidity added to both sides ($AMOUNT tokens each)."
