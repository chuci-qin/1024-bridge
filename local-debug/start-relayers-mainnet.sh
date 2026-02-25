#!/usr/bin/env bash
# start-relayers-mainnet.sh
# Start 3 relayers (9 processes) for Ethereum Mainnet <-> 1024chain Mainnet bridge
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DEPLOYMENT_FILE="$SCRIPT_DIR/deployment-mainnet.json"
BRIDGES_FILE="$PROJECT_ROOT/deploy/config/bridges.json"
ENV_DIR="$SCRIPT_DIR/env-mainnet"
LOG_DIR="$SCRIPT_DIR/logs-mainnet"
PID_DIR="$SCRIPT_DIR"

BRIDGE_ID="ethmain-1024main-usdc"

log() { echo "[relayer-mainnet][$(date '+%H:%M:%S')] $*"; }
die() { echo "[relayer-mainnet] ERROR: $*" >&2; exit 1; }

[ -f "$DEPLOYMENT_FILE" ] || die "deployment-mainnet.json not found. Run deploy-mainnet.sh first."

EVM_CONTRACT=$(jq -r '.evm_contract_address' "$DEPLOYMENT_FILE")
SVM_PROGRAM=$(jq -r '.svm_program_id' "$DEPLOYMENT_FILE")

BRIDGE_CFG=$(jq -r ".\"$BRIDGE_ID\"" "$BRIDGES_FILE")
EVM_NAME=$(echo "$BRIDGE_CFG" | jq -r '.evm.name')
EVM_CHAIN_ID=$(echo "$BRIDGE_CFG" | jq -r '.evm.chain_id')
EVM_RPC=$(echo "$BRIDGE_CFG" | jq -r '.evm.rpc_url')
EVM_TOKEN=$(echo "$BRIDGE_CFG" | jq -r '.evm.token_address')
EVM_CONFIRMS=$(echo "$BRIDGE_CFG" | jq -r '.evm.confirmation_blocks')
SVM_NAME=$(echo "$BRIDGE_CFG" | jq -r '.svm.name')
SVM_CHAIN_ID=$(echo "$BRIDGE_CFG" | jq -r '.svm.chain_id')
SVM_RPC=$(echo "$BRIDGE_CFG" | jq -r '.svm.rpc_url')
SVM_TOKEN=$(echo "$BRIDGE_CFG" | jq -r '.svm.token_address')
SVM_COMMIT=$(echo "$BRIDGE_CFG" | jq -r '.svm.commitment')
SVM_WS_URL=""

S2E_BIN="$PROJECT_ROOT/relayer/s2e/target/release/s2e-relayer"
E2S_LISTENER_BIN="$PROJECT_ROOT/relayer/e2s-listener/target/release/e2s-listener"
E2S_SUBMITTER_BIN="$PROJECT_ROOT/relayer/e2s-submitter/target/release/e2s-submitter"

for bin in "$S2E_BIN" "$E2S_LISTENER_BIN" "$E2S_SUBMITTER_BIN"; do
  [ -f "$bin" ] || die "Binary not found: $bin. Run 'cargo build --release' first."
done

mkdir -p "$ENV_DIR" "$LOG_DIR"

log "=========================================="
log "  Starting 3 Mainnet Relayers (9 processes)"
log "=========================================="
log "EVM Contract: $EVM_CONTRACT"
log "SVM Program:  $SVM_PROGRAM"
log ""

RELAYER_ENVS=(
  "$PROJECT_ROOT/relayer/easypanel-envs/ethmain-relayer1.env"
  "$PROJECT_ROOT/relayer/easypanel-envs/ethmain-relayer2.env"
  "$PROJECT_ROOT/relayer/easypanel-envs/ethmain-relayer3.env"
)

ALL_PIDS=""

for i in 0 1 2; do
  N=$((i + 1))
  S2E_PORT=$((9081 + i * 100))
  SUBMITTER_PORT=$((9082 + i * 100))
  LISTENER_PORT=$((9083 + i * 100))

  RELAYER_ENV="${RELAYER_ENVS[$i]}"
  [ -f "$RELAYER_ENV" ] || die "Relayer env not found: $RELAYER_ENV"

  source "$RELAYER_ENV"
  ECDSA_KEY="$RELAYER_ECDSA_PRIVATE_KEY"
  ED25519_KEY="$RELAYER_ED25519_PRIVATE_KEY"

  QUEUE_DIR="$SCRIPT_DIR/queue-mainnet/relayer$N"
  mkdir -p "$QUEUE_DIR"

  log "--- Relayer $N (s2e=$S2E_PORT, submitter=$SUBMITTER_PORT, listener=$LISTENER_PORT) ---"

  S2E_ENV="$ENV_DIR/relayer${N}-s2e.env"
  cat > "$S2E_ENV" <<EOF
SERVICE__NAME="s2e"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$SVM_NAME"
SOURCE_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$SVM_RPC"
# SOURCE_CHAIN__WS_URL not set — S2E uses HTTP polling fallback
SOURCE_CHAIN__CONTRACT_ADDRESS="$SVM_PROGRAM"
SOURCE_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__NAME="$EVM_NAME"
TARGET_CHAIN__CHAIN_ID="$EVM_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$EVM_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$EVM_CONTRACT"
TARGET_CHAIN__CONFIRMATION_BLOCKS="$EVM_CONFIRMS"
TARGET_CHAIN__USDC_MINT="$EVM_TOKEN"
RELAYER__ECDSA_PRIVATE_KEY="$ECDSA_KEY"
API__PORT="$S2E_PORT"
LOGGING__LEVEL="info"
LOGGING__FORMAT="text"
EOF

  LISTENER_ENV="$ENV_DIR/relayer${N}-e2s-listener.env"
  cat > "$LISTENER_ENV" <<EOF
SERVICE__NAME="e2s-listener"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$EVM_NAME"
SOURCE_CHAIN__CHAIN_ID="$EVM_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$EVM_RPC"
SOURCE_CHAIN__CONTRACT_ADDRESS="$EVM_CONTRACT"
SOURCE_CHAIN__CONFIRMATION_BLOCKS="$EVM_CONFIRMS"
TARGET_CHAIN__NAME="$SVM_NAME"
TARGET_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$SVM_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$SVM_PROGRAM"
TARGET_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__USDC_MINT="$SVM_TOKEN"
QUEUE__PATH="$QUEUE_DIR"
API__PORT="$LISTENER_PORT"
LOGGING__LEVEL="info"
LOGGING__FORMAT="text"
EOF

  SUBMITTER_ENV="$ENV_DIR/relayer${N}-e2s-submitter.env"
  cat > "$SUBMITTER_ENV" <<EOF
SERVICE__NAME="e2s-submitter"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$EVM_NAME"
SOURCE_CHAIN__CHAIN_ID="$EVM_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$EVM_RPC"
SOURCE_CHAIN__CONTRACT_ADDRESS="$EVM_CONTRACT"
SOURCE_CHAIN__CONFIRMATION_BLOCKS="$EVM_CONFIRMS"
TARGET_CHAIN__NAME="$SVM_NAME"
TARGET_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$SVM_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$SVM_PROGRAM"
TARGET_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__USDC_MINT="$SVM_TOKEN"
RELAYER__ED25519_PRIVATE_KEY="$ED25519_KEY"
QUEUE__PATH="$QUEUE_DIR"
API__PORT="$SUBMITTER_PORT"
LOGGING__LEVEL="info"
LOGGING__FORMAT="text"
EOF

  (cd "$(dirname "$S2E_BIN")" && set -a && . "$S2E_ENV" && set +a && exec "$S2E_BIN") \
    > "$LOG_DIR/relayer${N}-s2e.log" 2>&1 &
  S2E_PID=$!
  log "  Started s2e (PID=$S2E_PID)"

  (cd "$(dirname "$E2S_LISTENER_BIN")" && set -a && . "$LISTENER_ENV" && set +a && exec "$E2S_LISTENER_BIN") \
    > "$LOG_DIR/relayer${N}-e2s-listener.log" 2>&1 &
  LISTENER_PID=$!
  log "  Started e2s-listener (PID=$LISTENER_PID)"

  (cd "$(dirname "$E2S_SUBMITTER_BIN")" && set -a && . "$SUBMITTER_ENV" && set +a && exec "$E2S_SUBMITTER_BIN") \
    > "$LOG_DIR/relayer${N}-e2s-submitter.log" 2>&1 &
  SUBMITTER_PID=$!
  log "  Started e2s-submitter (PID=$SUBMITTER_PID)"

  ALL_PIDS="$ALL_PIDS $S2E_PID $LISTENER_PID $SUBMITTER_PID"
done

echo "$ALL_PIDS" | tr ' ' '\n' | grep -v '^$' > "$PID_DIR/relayers-mainnet.pid"
log ""
log "All 9 mainnet processes started. PIDs saved to $PID_DIR/relayers-mainnet.pid"
log "Logs in: $LOG_DIR/"
log ""
log "To stop: kill \$(cat $PID_DIR/relayers-mainnet.pid)"
log "To check health: curl http://localhost:9081/health"
