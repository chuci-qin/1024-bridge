#!/usr/bin/env bash
# test-e2s.sh — E2E test: EVM -> SVM direction
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOYMENT_FILE="$SCRIPT_DIR/deployment.json"

[ -f "$DEPLOYMENT_FILE" ] || { echo "ERROR: deployment.json not found"; exit 1; }

ADMIN_EVM_KEY=$(jq -r '.private_key' "$PROJECT_ROOT/deploy/keys/admin-evm-private-key.json")

cd "$PROJECT_ROOT/deploy/scripts"

ADMIN_KEYPAIR_PATH="$PROJECT_ROOT/deploy/keys/admin-svm-keypair.json" \
ADMIN_EVM_PRIVATE_KEY="$ADMIN_EVM_KEY" \
EVM_RPC_URL="$(jq -r '.evm_rpc_url' "$DEPLOYMENT_FILE")" \
SVM_RPC_URL="$(jq -r '.svm_rpc_url' "$DEPLOYMENT_FILE")" \
EVM_CONTRACT_ADDRESS="$(jq -r '.evm_contract_address' "$DEPLOYMENT_FILE")" \
SVM_PROGRAM_ID="$(jq -r '.svm_program_id' "$DEPLOYMENT_FILE")" \
EVM_TOKEN_ADDRESS="$(jq -r '.evm_token_address' "$DEPLOYMENT_FILE")" \
SVM_TOKEN_ADDRESS="$(jq -r '.svm_token_address' "$DEPLOYMENT_FILE")" \
IDL_PATH="$PROJECT_ROOT/svm/bridge1024/target/idl/bridge1024.json" \
TEST_AMOUNT="${1:-10000}" \
TIMEOUT_MS="${TIMEOUT_MS:-120000}" \
INITIAL_DELAY_MS="${INITIAL_DELAY_MS:-10000}" \
POLL_INTERVAL_MS="${POLL_INTERVAL_MS:-5000}" \
  npx ts-node e2e-evm-to-svm.ts
