#!/usr/bin/env bash
#
# test-e2e-sol-svm-localnet.sh
#
# Full local E2E test: Solana <-> 1024chain bridge
#
# Spins up two solana-test-validator instances (simulating Solana and 1024chain),
# deploys both bridge programs, creates mock USDC, registers relayers,
# adds liquidity, builds & starts relayer processes, then optionally triggers
# a test stake to verify cross-chain transfer.
#
# Port layout:
#   Validator 1 (Solana):    RPC 8899, WS 8900, faucet 8990
#   Validator 2 (1024chain): RPC 9899, WS 9900, faucet 9990
#
# Usage:
#   cd local-debug && bash test-e2e-sol-svm-localnet.sh
#
# Prerequisites:
#   - solana CLI, anchor CLI, spl-token CLI, cargo, jq installed
#   - yarn install done in solana/bridge1024, svm/bridge1024, deploy/scripts

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SOLANA_DIR="$REPO_ROOT/solana/bridge1024"
SVM_DIR="$REPO_ROOT/svm/bridge1024"
DEPLOY_SCRIPTS_DIR="$REPO_ROOT/deploy/scripts"
ADMIN_SOL_KEYPAIR="$REPO_ROOT/deploy/keys/admin-solana-keypair.json"
ADMIN_SVM_KEYPAIR="$REPO_ROOT/deploy/keys/admin-svm-keypair.json"
RELAYERS_FILE="$REPO_ROOT/deploy/keys/relayers.json"

LOCALNET_DIR="$SCRIPT_DIR/localnet-e2e"
LEDGER_SOL="$LOCALNET_DIR/ledger-solana"
LEDGER_SVM="$LOCALNET_DIR/ledger-svm"
RELAYER_KEYS_DIR="$LOCALNET_DIR/relayer-keys"
RELAYER_ENV_DIR="$LOCALNET_DIR/env"
RELAYER_LOG_DIR="$LOCALNET_DIR/logs"
RELAYER_QUEUE_DIR="$LOCALNET_DIR/queue"
DEPLOYMENT_FILE="$LOCALNET_DIR/deployment.json"

SOL_RPC="http://127.0.0.1:8899"
SOL_WS="ws://127.0.0.1:8900"
SVM_RPC="http://127.0.0.1:9899"
SVM_WS="ws://127.0.0.1:9900"

SOL_CHAIN_ID=103
SVM_CHAIN_ID=91024

log() { echo "[e2e][$(date '+%H:%M:%S')] $*"; }
die() { echo "[e2e] ERROR: $*" >&2; exit 1; }

cleanup() {
    log "Cleaning up..."
    [ -f "$LOCALNET_DIR/pids.txt" ] && while read -r pid; do
        kill "$pid" 2>/dev/null || true
    done < "$LOCALNET_DIR/pids.txt"
    pkill -f "solana-test-validator.*ledger-solana" 2>/dev/null || true
    pkill -f "solana-test-validator.*ledger-svm" 2>/dev/null || true
    log "Cleanup done."
}
trap cleanup EXIT

mkdir -p "$LOCALNET_DIR" "$RELAYER_KEYS_DIR" "$RELAYER_ENV_DIR" "$RELAYER_LOG_DIR" "$RELAYER_QUEUE_DIR"
> "$LOCALNET_DIR/pids.txt"

log "================================================================"
log "  Local E2E Test: Solana <-> 1024chain Bridge"
log "================================================================"

# ============================================================
# Step 1: Verify keypairs exist
# ============================================================
log ""
log ">>> Step 1: Verify admin keypairs..."
[ -f "$ADMIN_SOL_KEYPAIR" ] || die "Solana admin keypair not found: $ADMIN_SOL_KEYPAIR"
[ -f "$ADMIN_SVM_KEYPAIR" ] || die "SVM admin keypair not found: $ADMIN_SVM_KEYPAIR"

ADMIN_SOL_PUBKEY=$(solana-keygen pubkey "$ADMIN_SOL_KEYPAIR")
ADMIN_SVM_PUBKEY=$(solana-keygen pubkey "$ADMIN_SVM_KEYPAIR")
log "Solana admin: $ADMIN_SOL_PUBKEY"
log "SVM admin:    $ADMIN_SVM_PUBKEY"

# ============================================================
# Step 2: Generate relayer ed25519 keypairs (3 relayers)
# ============================================================
log ""
log ">>> Step 2: Generate relayer keypairs..."
RELAYER_PUBKEYS=()
RELAYER_SEEDS_HEX=()

for i in 1 2 3; do
    KEYFILE="$RELAYER_KEYS_DIR/relayer${i}.json"
    if [ ! -f "$KEYFILE" ]; then
        solana-keygen new --no-bip39-passphrase -o "$KEYFILE" --force 2>/dev/null
    fi
    PUBKEY=$(solana-keygen pubkey "$KEYFILE")
    RELAYER_PUBKEYS+=("$PUBKEY")

    # Extract first 32 bytes (seed) as hex for relayer config
    SEED_HEX=$(python3 -c "
import json, sys
data = json.load(open('$KEYFILE'))
print(''.join(f'{b:02x}' for b in data[:32]))
")
    RELAYER_SEEDS_HEX+=("$SEED_HEX")
    log "  Relayer $i: pubkey=$PUBKEY"
done

# Write localnet relayers.json
cat > "$LOCALNET_DIR/relayers.json" <<EOF
{
  "relayers": [
    { "name": "relayer1", "evm_address": "0x0000000000000000000000000000000000000001", "svm_pubkey": "${RELAYER_PUBKEYS[0]}" },
    { "name": "relayer2", "evm_address": "0x0000000000000000000000000000000000000002", "svm_pubkey": "${RELAYER_PUBKEYS[1]}" },
    { "name": "relayer3", "evm_address": "0x0000000000000000000000000000000000000003", "svm_pubkey": "${RELAYER_PUBKEYS[2]}" }
  ]
}
EOF
RELAYERS_FILE_LOCAL="$LOCALNET_DIR/relayers.json"

# ============================================================
# Step 3: Start two validators
# ============================================================
log ""
log ">>> Step 3: Starting two solana-test-validator instances..."

pkill -f "solana-test-validator" 2>/dev/null || true
sleep 3

# Validator 1: Solana (RPC 8899, WS 8900, faucet 8990)
log "  Starting Validator 1 (Solana)..."
solana-test-validator \
    --reset --quiet \
    --rpc-port 8899 \
    --faucet-port 8990 \
    --ledger "$LEDGER_SOL" &
V1_PID=$!
echo "$V1_PID" >> "$LOCALNET_DIR/pids.txt"
log "  Validator 1 PID=$V1_PID, RPC=$SOL_RPC, WS=$SOL_WS"

# Wait for validator 1 before starting validator 2
log "  Waiting for Validator 1..."
for attempt in $(seq 1 12); do
    solana cluster-version -u "$SOL_RPC" &>/dev/null && break
    [ "$attempt" -eq 12 ] && die "Validator 1 failed to start after 60s"
    sleep 5
done
log "  Validator 1 is up."

# Validator 2: 1024chain (RPC 9899, WS 9900, faucet 9990)
# Use separate gossip port and dynamic port range to avoid conflicts with Validator 1
log "  Starting Validator 2 (1024chain)..."
solana-test-validator \
    --reset --quiet \
    --rpc-port 9899 \
    --faucet-port 9990 \
    --gossip-port 10015 \
    --dynamic-port-range 10016-10100 \
    --ledger "$LEDGER_SVM" &
V2_PID=$!
echo "$V2_PID" >> "$LOCALNET_DIR/pids.txt"
log "  Validator 2 PID=$V2_PID, RPC=$SVM_RPC, WS=$SVM_WS"

log "  Waiting for Validator 2..."
for attempt in $(seq 1 12); do
    solana cluster-version -u "$SVM_RPC" &>/dev/null && break
    [ "$attempt" -eq 12 ] && die "Validator 2 failed to start after 60s"
    sleep 5
done
log "  Validator 2 is up."
log "  Both validators running."

# ============================================================
# Step 4: Airdrop SOL to admins and relayers on both chains
# ============================================================
log ""
log ">>> Step 4: Airdrop SOL..."

# Solana side
solana config set --url "$SOL_RPC" --keypair "$ADMIN_SOL_KEYPAIR" &>/dev/null
solana airdrop 100 "$ADMIN_SOL_PUBKEY" --url "$SOL_RPC" 2>/dev/null || true
for pub in "${RELAYER_PUBKEYS[@]}"; do
    solana airdrop 10 "$pub" --url "$SOL_RPC" 2>/dev/null || true
done
log "  Solana: admin + 3 relayers funded"

# SVM side
solana config set --url "$SVM_RPC" --keypair "$ADMIN_SVM_KEYPAIR" &>/dev/null
solana airdrop 100 "$ADMIN_SVM_PUBKEY" --url "$SVM_RPC" 2>/dev/null || true
for pub in "${RELAYER_PUBKEYS[@]}"; do
    solana airdrop 10 "$pub" --url "$SVM_RPC" 2>/dev/null || true
done
log "  SVM: admin + 3 relayers funded"

# ============================================================
# Step 5: Build & deploy Solana bridge program
# ============================================================
log ""
log ">>> Step 5: Build & deploy Solana bridge..."
cd "$SOLANA_DIR"

if grep -q 'cluster = "devnet"' Anchor.toml; then
    sed -i 's/cluster = "devnet"/cluster = "localnet"/' Anchor.toml
fi
sed -i "s|wallet = .*|wallet = \"$ADMIN_SOL_KEYPAIR\"|" Anchor.toml

solana config set --url "$SOL_RPC" --keypair "$ADMIN_SOL_KEYPAIR" &>/dev/null
anchor keys sync 2>&1 | tail -5
anchor build 2>&1 | tail -3
anchor deploy --provider.cluster localnet --provider.wallet "$ADMIN_SOL_KEYPAIR" 2>&1 | tail -5

SOL_PROGRAM_ID=$(solana-keygen pubkey "$SOLANA_DIR/target/deploy/bridge1024_solana-keypair.json")
log "  Solana Program ID: $SOL_PROGRAM_ID"

# ============================================================
# Step 6: Build & deploy SVM bridge program
# ============================================================
log ""
log ">>> Step 6: Build & deploy SVM bridge (1024chain)..."
cd "$SVM_DIR"

sed -i "s|wallet = .*|wallet = \"$ADMIN_SVM_KEYPAIR\"|" Anchor.toml

# Point solana CLI to SVM validator before deploy
# NOTE: --provider.cluster localnet hardcodes to 8899 in Anchor,
# so we use the actual URL instead
solana config set --url "$SVM_RPC" --keypair "$ADMIN_SVM_KEYPAIR" &>/dev/null
anchor keys sync 2>&1 | tail -5
anchor build 2>&1 | tail -3
anchor deploy --provider.cluster "$SVM_RPC" --provider.wallet "$ADMIN_SVM_KEYPAIR" 2>&1 | tail -5

SVM_PROGRAM_ID=$(solana-keygen pubkey "$SVM_DIR/target/deploy/bridge1024-keypair.json")
log "  SVM Program ID: $SVM_PROGRAM_ID"

# ============================================================
# Step 7: Create mock USDC on both chains
# ============================================================
log ""
log ">>> Step 7: Create mock USDC mints..."

# Solana USDC
solana config set --url "$SOL_RPC" --keypair "$ADMIN_SOL_KEYPAIR" &>/dev/null
SOL_USDC_RESULT=$(spl-token create-token --decimals 6 --url "$SOL_RPC" 2>&1)
SOL_USDC_MINT=$(echo "$SOL_USDC_RESULT" | grep "Creating token" | awk '{print $3}')
[ -n "$SOL_USDC_MINT" ] || die "Failed to create Solana USDC mint"
spl-token create-account "$SOL_USDC_MINT" --url "$SOL_RPC" 2>&1 || true
spl-token mint "$SOL_USDC_MINT" 10000000000 --url "$SOL_RPC" 2>&1 || true
log "  Solana USDC: $SOL_USDC_MINT (minted 10,000 USDC)"

# SVM USDC
solana config set --url "$SVM_RPC" --keypair "$ADMIN_SVM_KEYPAIR" &>/dev/null
SVM_USDC_RESULT=$(spl-token create-token --decimals 6 --url "$SVM_RPC" 2>&1)
SVM_USDC_MINT=$(echo "$SVM_USDC_RESULT" | grep "Creating token" | awk '{print $3}')
[ -n "$SVM_USDC_MINT" ] || die "Failed to create SVM USDC mint"
spl-token create-account "$SVM_USDC_MINT" --url "$SVM_RPC" 2>&1 || true
spl-token mint "$SVM_USDC_MINT" 10000000000 --url "$SVM_RPC" 2>&1 || true
log "  SVM USDC: $SVM_USDC_MINT (minted 10,000 USDC)"

# ============================================================
# Step 8: Initialize & configure both programs
# ============================================================
log ""
log ">>> Step 8: Initialize & configure programs..."
cd "$DEPLOY_SCRIPTS_DIR"
[ -d "node_modules" ] || npm install

# Initialize Solana bridge
log "  --- Solana bridge init ---"
ADMIN_KEYPAIR_PATH="$ADMIN_SOL_KEYPAIR" \
PROGRAM_ID="$SOL_PROGRAM_ID" \
SOLANA_RPC_URL="$SOL_RPC" \
USDC_MINT="$SOL_USDC_MINT" \
PEER_CONTRACT="$SVM_PROGRAM_ID" \
SOURCE_CHAIN_ID="$SOL_CHAIN_ID" \
TARGET_CHAIN_ID="$SVM_CHAIN_ID" \
LIQUIDITY_AMOUNT="5000000000" \
SKIP_LIQUIDITY="false" \
RELAYERS_FILE="$RELAYERS_FILE_LOCAL" \
IDL_PATH="$SOLANA_DIR/target/idl/bridge1024_solana.json" \
  npx ts-node deploy-and-init-solana.ts 2>&1 | tail -20

# Initialize SVM bridge
log "  --- SVM bridge init ---"
ADMIN_KEYPAIR_PATH="$ADMIN_SVM_KEYPAIR" \
PROGRAM_ID="$SVM_PROGRAM_ID" \
SVM_RPC_URL="$SVM_RPC" \
USDC_MINT="$SVM_USDC_MINT" \
PEER_CONTRACT="$SOL_PROGRAM_ID" \
SOURCE_CHAIN_ID="$SVM_CHAIN_ID" \
TARGET_CHAIN_ID="$SOL_CHAIN_ID" \
LIQUIDITY_AMOUNT="5000000000" \
SKIP_LIQUIDITY="false" \
BRIDGE_FEE="0" \
RELAYERS_FILE="$RELAYERS_FILE_LOCAL" \
IDL_PATH="$SVM_DIR/target/idl/bridge1024.json" \
  npx ts-node deploy-and-init-svm.ts 2>&1 | tail -20

# ============================================================
# Step 9: Build relayer binaries
# ============================================================
log ""
log ">>> Step 9: Build relayer binaries..."
cd "$REPO_ROOT/relayer"

for component in sol2svm-listener sol2svm-submitter svm2sol-listener svm2sol-submitter; do
    BIN_PATH="$component/target/release/$component"
    if [ -f "$BIN_PATH" ]; then
        log "  $component: already built"
    else
        log "  Building $component (this may take a while)..."
        (cd "$component" && cargo build --release 2>&1 | tail -3)
        log "  $component: built"
    fi
done

SOL2SVM_LISTENER_BIN="$REPO_ROOT/relayer/sol2svm-listener/target/release/sol2svm-listener"
SOL2SVM_SUBMITTER_BIN="$REPO_ROOT/relayer/sol2svm-submitter/target/release/sol2svm-submitter"
SVM2SOL_LISTENER_BIN="$REPO_ROOT/relayer/svm2sol-listener/target/release/svm2sol-listener"
SVM2SOL_SUBMITTER_BIN="$REPO_ROOT/relayer/svm2sol-submitter/target/release/svm2sol-submitter"

# ============================================================
# Step 10: Generate relayer .env files & start processes
# ============================================================
log ""
log ">>> Step 10: Start relayers (3 relayers × 4 processes = 12 total)..."

for i in 0 1 2; do
    N=$((i + 1))
    SOL2SVM_LISTENER_PORT=$((7001 + i * 10))
    SOL2SVM_SUBMITTER_PORT=$((7002 + i * 10))
    SVM2SOL_LISTENER_PORT=$((7003 + i * 10))
    SVM2SOL_SUBMITTER_PORT=$((7004 + i * 10))

    QUEUE_SOL2SVM="$RELAYER_QUEUE_DIR/relayer${N}/sol2svm"
    QUEUE_SVM2SOL="$RELAYER_QUEUE_DIR/relayer${N}/svm2sol"
    mkdir -p "$QUEUE_SOL2SVM" "$QUEUE_SVM2SOL"

    ED25519_KEY="${RELAYER_SEEDS_HEX[$i]}"

    # --- sol2svm-listener env ---
    cat > "$RELAYER_ENV_DIR/relayer${N}-sol2svm-listener.env" <<EOF
SERVICE__NAME=sol2svm-listener
SERVICE__VERSION=0.1.0
SERVICE__WORKER_POOL_SIZE=5
SOURCE_CHAIN__NAME=Solana-Localnet
SOURCE_CHAIN__CHAIN_ID=$SOL_CHAIN_ID
SOURCE_CHAIN__RPC_URL=$SOL_RPC
SOURCE_CHAIN__CONTRACT_ADDRESS=$SOL_PROGRAM_ID
SOURCE_CHAIN__COMMITMENT=confirmed
TARGET_CHAIN__NAME=1024chain-Localnet
TARGET_CHAIN__CHAIN_ID=$SVM_CHAIN_ID
TARGET_CHAIN__RPC_URL=$SVM_RPC
TARGET_CHAIN__CONTRACT_ADDRESS=$SVM_PROGRAM_ID
TARGET_CHAIN__COMMITMENT=confirmed
TARGET_CHAIN__USDC_MINT=$SVM_USDC_MINT
QUEUE__PATH=$QUEUE_SOL2SVM
API__PORT=$SOL2SVM_LISTENER_PORT
LOGGING__LEVEL=info
LOGGING__FORMAT=text
EOF

    # --- sol2svm-submitter env ---
    cat > "$RELAYER_ENV_DIR/relayer${N}-sol2svm-submitter.env" <<EOF
SERVICE__NAME=sol2svm-submitter
SERVICE__VERSION=0.1.0
SERVICE__WORKER_POOL_SIZE=5
SOURCE_CHAIN__NAME=Solana-Localnet
SOURCE_CHAIN__CHAIN_ID=$SOL_CHAIN_ID
SOURCE_CHAIN__RPC_URL=$SOL_RPC
SOURCE_CHAIN__CONTRACT_ADDRESS=$SOL_PROGRAM_ID
SOURCE_CHAIN__COMMITMENT=confirmed
TARGET_CHAIN__NAME=1024chain-Localnet
TARGET_CHAIN__CHAIN_ID=$SVM_CHAIN_ID
TARGET_CHAIN__RPC_URL=$SVM_RPC
TARGET_CHAIN__CONTRACT_ADDRESS=$SVM_PROGRAM_ID
TARGET_CHAIN__COMMITMENT=confirmed
TARGET_CHAIN__USDC_MINT=$SVM_USDC_MINT
RELAYER__ED25519_PRIVATE_KEY=$ED25519_KEY
QUEUE__PATH=$QUEUE_SOL2SVM
API__PORT=$SOL2SVM_SUBMITTER_PORT
LOGGING__LEVEL=info
LOGGING__FORMAT=text
EOF

    # --- svm2sol-listener env (uses WebSocket!) ---
    cat > "$RELAYER_ENV_DIR/relayer${N}-svm2sol-listener.env" <<EOF
SERVICE__NAME=svm2sol-listener
SERVICE__VERSION=0.1.0
SERVICE__WORKER_POOL_SIZE=5
SOURCE_CHAIN__NAME=1024chain-Localnet
SOURCE_CHAIN__CHAIN_ID=$SVM_CHAIN_ID
SOURCE_CHAIN__RPC_URL=$SVM_RPC
SOURCE_CHAIN__WS_URL=$SVM_WS
SOURCE_CHAIN__CONTRACT_ADDRESS=$SVM_PROGRAM_ID
SOURCE_CHAIN__COMMITMENT=confirmed
TARGET_CHAIN__NAME=Solana-Localnet
TARGET_CHAIN__CHAIN_ID=$SOL_CHAIN_ID
TARGET_CHAIN__RPC_URL=$SOL_RPC
TARGET_CHAIN__CONTRACT_ADDRESS=$SOL_PROGRAM_ID
TARGET_CHAIN__COMMITMENT=confirmed
TARGET_CHAIN__USDC_MINT=$SOL_USDC_MINT
QUEUE__PATH=$QUEUE_SVM2SOL
API__PORT=$SVM2SOL_LISTENER_PORT
LOGGING__LEVEL=info
LOGGING__FORMAT=text
EOF

    # --- svm2sol-submitter env ---
    cat > "$RELAYER_ENV_DIR/relayer${N}-svm2sol-submitter.env" <<EOF
SERVICE__NAME=svm2sol-submitter
SERVICE__VERSION=0.1.0
SERVICE__WORKER_POOL_SIZE=5
SOURCE_CHAIN__NAME=1024chain-Localnet
SOURCE_CHAIN__CHAIN_ID=$SVM_CHAIN_ID
SOURCE_CHAIN__RPC_URL=$SVM_RPC
SOURCE_CHAIN__CONTRACT_ADDRESS=$SVM_PROGRAM_ID
SOURCE_CHAIN__COMMITMENT=confirmed
TARGET_CHAIN__NAME=Solana-Localnet
TARGET_CHAIN__CHAIN_ID=$SOL_CHAIN_ID
TARGET_CHAIN__RPC_URL=$SOL_RPC
TARGET_CHAIN__CONTRACT_ADDRESS=$SOL_PROGRAM_ID
TARGET_CHAIN__COMMITMENT=confirmed
TARGET_CHAIN__USDC_MINT=$SOL_USDC_MINT
RELAYER__ED25519_PRIVATE_KEY=$ED25519_KEY
QUEUE__PATH=$QUEUE_SVM2SOL
API__PORT=$SVM2SOL_SUBMITTER_PORT
LOGGING__LEVEL=info
LOGGING__FORMAT=text
EOF

    # Start 4 processes for this relayer
    for comp_env in \
        "sol2svm-listener:$SOL2SVM_LISTENER_BIN:$RELAYER_ENV_DIR/relayer${N}-sol2svm-listener.env" \
        "sol2svm-submitter:$SOL2SVM_SUBMITTER_BIN:$RELAYER_ENV_DIR/relayer${N}-sol2svm-submitter.env" \
        "svm2sol-listener:$SVM2SOL_LISTENER_BIN:$RELAYER_ENV_DIR/relayer${N}-svm2sol-listener.env" \
        "svm2sol-submitter:$SVM2SOL_SUBMITTER_BIN:$RELAYER_ENV_DIR/relayer${N}-svm2sol-submitter.env"
    do
        COMP_NAME="${comp_env%%:*}"
        REST="${comp_env#*:}"
        BIN="${REST%%:*}"
        ENV_FILE="${REST#*:}"

        (set -a && . "$ENV_FILE" && set +a && exec "$BIN") \
            > "$RELAYER_LOG_DIR/relayer${N}-${COMP_NAME}.log" 2>&1 &
        COMP_PID=$!
        echo "$COMP_PID" >> "$LOCALNET_DIR/pids.txt"
        log "  Relayer $N $COMP_NAME started (PID=$COMP_PID)"
    done
done

# ============================================================
# Step 11: Save deployment info
# ============================================================
jq -n \
  --arg sol_program "$SOL_PROGRAM_ID" \
  --arg svm_program "$SVM_PROGRAM_ID" \
  --arg sol_rpc "$SOL_RPC" \
  --arg svm_rpc "$SVM_RPC" \
  --arg sol_ws "$SOL_WS" \
  --arg svm_ws "$SVM_WS" \
  --arg sol_usdc "$SOL_USDC_MINT" \
  --arg svm_usdc "$SVM_USDC_MINT" \
  --arg sol_admin "$ADMIN_SOL_PUBKEY" \
  --arg svm_admin "$ADMIN_SVM_PUBKEY" \
  '{
    solana_program_id: $sol_program,
    svm_program_id: $svm_program,
    solana_rpc: $sol_rpc,
    svm_rpc: $svm_rpc,
    solana_ws: $sol_ws,
    svm_ws: $svm_ws,
    solana_usdc_mint: $sol_usdc,
    svm_usdc_mint: $svm_usdc,
    solana_admin: $sol_admin,
    svm_admin: $svm_admin
  }' > "$DEPLOYMENT_FILE"

log ""
log "================================================================"
log "  Local E2E Environment Ready!"
log "================================================================"
log ""
log "  Solana:     RPC=$SOL_RPC  WS=$SOL_WS"
log "  1024chain:  RPC=$SVM_RPC  WS=$SVM_WS"
log ""
log "  Solana Program:  $SOL_PROGRAM_ID"
log "  SVM Program:     $SVM_PROGRAM_ID"
log "  Solana USDC:     $SOL_USDC_MINT"
log "  SVM USDC:        $SVM_USDC_MINT"
log ""
log "  12 relayer processes running (3 relayers × 4 components)"
log "  Logs: $RELAYER_LOG_DIR/"
log "  Deployment info: $DEPLOYMENT_FILE"
log ""
log "  Press Ctrl+C to stop everything."
log ""

# Keep script alive, tail relayer logs
tail -f "$RELAYER_LOG_DIR"/*.log 2>/dev/null || wait
