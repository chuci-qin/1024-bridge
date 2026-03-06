#!/usr/bin/env bash
#
# test-e2e-sol-svm1-localnet.sh
#
# Full local E2E test: Solana <-> 1024chain bridge (unified contract)
#
# Both chains deploy the same bridge1024 program. Solana has bridge_fee=0,
# 1024chain has bridge_fee>0. The test spins up two validators, deploys
# the unified contract to both, configures relayers, and starts relayer
# processes inside Docker containers (matching production deployment).
#
# Port layout:
#   Validator 1 (Solana):    RPC 8899, WS 8900, faucet 8990
#   Validator 2 (1024chain): RPC 9899, WS 9900, faucet 9990
#
# Usage:
#   cd local-debug && bash test-e2e-sol-svm1-localnet.sh
#
# Prerequisites:
#   - solana CLI, anchor CLI, spl-token CLI, docker, jq installed
#   - npm install done in solana/bridge1024 and deploy1/scripts

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACT_DIR="$REPO_ROOT/solana/bridge1024"
DEPLOY_SCRIPTS_DIR="$REPO_ROOT/deploy1/scripts"
# Both chains use the same admin keypair because the unified contract has
# HARDCODED_ADMIN = 2XVdXwC235qFXSm5egXpWyNY9xaiShFD5HKGrEhQNEFY.
# In production, the contract would be compiled with a different HARDCODED_ADMIN per chain.
ADMIN_SOL_KEYPAIR="$REPO_ROOT/deploy1/keys/admin-svm-keypair.json"
ADMIN_SVM_KEYPAIR="$REPO_ROOT/deploy1/keys/admin-svm-keypair.json"
RELAYERS_FILE="$REPO_ROOT/deploy1/keys/relayers.json"

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
SVM_BRIDGE_FEE=1000

log() { echo "[e2e][$(date '+%H:%M:%S')] $*"; }
die() { echo "[e2e] ERROR: $*" >&2; exit 1; }

DOCKER_IMAGE="bridge1024-relayer-local"
DOCKER_CONTAINERS=()

cleanup() {
    log "Cleaning up..."
    for cname in "${DOCKER_CONTAINERS[@]}"; do
        docker rm -f "$cname" 2>/dev/null || true
    done
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
log "  Local E2E Test: Solana <-> 1024chain Bridge (Unified Contract)"
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
    { "name": "relayer1", "svm_pubkey": "${RELAYER_PUBKEYS[0]}" },
    { "name": "relayer2", "svm_pubkey": "${RELAYER_PUBKEYS[1]}" },
    { "name": "relayer3", "svm_pubkey": "${RELAYER_PUBKEYS[2]}" }
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

log "  Waiting for Validator 1..."
for attempt in $(seq 1 12); do
    solana cluster-version -u "$SOL_RPC" &>/dev/null && break
    [ "$attempt" -eq 12 ] && die "Validator 1 failed to start after 60s"
    sleep 5
done
log "  Validator 1 is up."

# Validator 2: 1024chain (RPC 9899, WS 9900, faucet 9990)
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
# Step 5: Build the unified contract
# ============================================================
log ""
log ">>> Step 5: Build the unified bridge contract..."
cd "$CONTRACT_DIR"

sed -i "s|wallet = .*|wallet = \"$ADMIN_SOL_KEYPAIR\"|" Anchor.toml

solana config set --url "$SOL_RPC" --keypair "$ADMIN_SOL_KEYPAIR" &>/dev/null
anchor keys sync 2>&1 | tail -5
anchor build 2>&1 | tail -3

BRIDGE_KEYPAIR="$CONTRACT_DIR/target/deploy/bridge1024-keypair.json"
BRIDGE_SO="$CONTRACT_DIR/target/deploy/bridge1024.so"
IDL_PATH="$CONTRACT_DIR/target/idl/bridge1024.json"

log "  Contract built: $BRIDGE_SO"
log "  IDL: $IDL_PATH"

# ============================================================
# Step 6: Deploy to Solana (Validator 1)
# ============================================================
log ""
log ">>> Step 6: Deploy bridge to Solana..."
solana config set --url "$SOL_RPC" --keypair "$ADMIN_SOL_KEYPAIR" &>/dev/null
anchor deploy --provider.cluster localnet --provider.wallet "$ADMIN_SOL_KEYPAIR" 2>&1 | tail -5

SOL_PROGRAM_ID=$(solana-keygen pubkey "$BRIDGE_KEYPAIR")
log "  Solana Program ID: $SOL_PROGRAM_ID"

# ============================================================
# Step 7: Deploy to 1024chain (Validator 2) using solana program deploy
# ============================================================
log ""
log ">>> Step 7: Deploy bridge to 1024chain (SVM)..."
solana config set --url "$SVM_RPC" --keypair "$ADMIN_SVM_KEYPAIR" &>/dev/null

# Deploy using solana CLI directly (Anchor's --provider.cluster localnet hardcodes port 8899)
solana program deploy "$BRIDGE_SO" \
    --program-id "$BRIDGE_KEYPAIR" \
    --url "$SVM_RPC" \
    --keypair "$ADMIN_SVM_KEYPAIR" 2>&1 | tail -5

SVM_PROGRAM_ID=$(solana-keygen pubkey "$BRIDGE_KEYPAIR")
log "  SVM Program ID: $SVM_PROGRAM_ID"

# ============================================================
# Step 8: Create mock USDC on both chains
# ============================================================
log ""
log ">>> Step 8: Create mock USDC mints..."

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
# Step 9: Initialize & configure both programs using unified deploy-and-init.ts
# ============================================================
log ""
log ">>> Step 9: Initialize & configure programs..."
cd "$DEPLOY_SCRIPTS_DIR"
[ -d "node_modules" ] || npm install

# Initialize Solana bridge (fee = 0)
log "  --- Solana bridge init (fee=0) ---"
ADMIN_KEYPAIR_PATH="$ADMIN_SOL_KEYPAIR" \
PROGRAM_ID="$SOL_PROGRAM_ID" \
RPC_URL="$SOL_RPC" \
USDC_MINT="$SOL_USDC_MINT" \
PEER_CONTRACT="$SVM_PROGRAM_ID" \
SOURCE_CHAIN_ID="$SOL_CHAIN_ID" \
TARGET_CHAIN_ID="$SVM_CHAIN_ID" \
LIQUIDITY_AMOUNT="5000000000" \
SKIP_LIQUIDITY="false" \
BRIDGE_FEE="0" \
RELAYERS_FILE="$RELAYERS_FILE_LOCAL" \
IDL_PATH="$IDL_PATH" \
  npx ts-node deploy-and-init.ts 2>&1 | tail -20

# Initialize SVM bridge (fee > 0)
log "  --- SVM bridge init (fee=$SVM_BRIDGE_FEE) ---"
ADMIN_KEYPAIR_PATH="$ADMIN_SVM_KEYPAIR" \
PROGRAM_ID="$SVM_PROGRAM_ID" \
RPC_URL="$SVM_RPC" \
USDC_MINT="$SVM_USDC_MINT" \
PEER_CONTRACT="$SOL_PROGRAM_ID" \
SOURCE_CHAIN_ID="$SVM_CHAIN_ID" \
TARGET_CHAIN_ID="$SOL_CHAIN_ID" \
LIQUIDITY_AMOUNT="5000000000" \
SKIP_LIQUIDITY="false" \
BRIDGE_FEE="$SVM_BRIDGE_FEE" \
RELAYERS_FILE="$RELAYERS_FILE_LOCAL" \
IDL_PATH="$IDL_PATH" \
  npx ts-node deploy-and-init.ts 2>&1 | tail -20

# ============================================================
# Step 10: Build Docker image for relayer
# ============================================================
log ""
log ">>> Step 10: Build relayer Docker image..."
cd "$REPO_ROOT"

if docker image inspect "$DOCKER_IMAGE" &>/dev/null; then
    log "  Image $DOCKER_IMAGE already exists. Rebuilding..."
fi
docker build -f relayer1/Dockerfile -t "$DOCKER_IMAGE" . 2>&1 | tail -5
log "  Docker image $DOCKER_IMAGE built."

# ============================================================
# Step 11: Generate localnet bridges.json & start Docker containers
# ============================================================
log ""
log ">>> Step 11: Start relayers in Docker (3 containers × 4 processes = 12 total)..."

LOCALNET_BRIDGES="$LOCALNET_DIR/bridges-localnet.json"
jq -n \
  --arg sol_name "Solana-Localnet" \
  --argjson sol_chain_id "$SOL_CHAIN_ID" \
  --arg sol_rpc "$SOL_RPC" \
  --arg sol_program "$SOL_PROGRAM_ID" \
  --arg sol_token "$SOL_USDC_MINT" \
  --arg svm_name "1024chain-Localnet" \
  --argjson svm_chain_id "$SVM_CHAIN_ID" \
  --arg svm_rpc "$SVM_RPC" \
  --arg svm_token "$SVM_USDC_MINT" \
  '{
    "localnet": {
      "token": "USDC",
      "solana": {
        "name": $sol_name,
        "chain_id": $sol_chain_id,
        "rpc_url": $sol_rpc,
        "program_id": $sol_program,
        "token_address": $sol_token,
        "commitment": "confirmed"
      },
      "svm": {
        "name": $svm_name,
        "chain_id": $svm_chain_id,
        "rpc_url": $svm_rpc,
        "token_address": $svm_token,
        "commitment": "confirmed"
      }
    }
  }' > "$LOCALNET_BRIDGES"
log "  Generated $LOCALNET_BRIDGES"

for i in 0 1 2; do
    N=$((i + 1))
    ED25519_KEY="${RELAYER_SEEDS_HEX[$i]}"
    CNAME="bridge1024-relayer-${N}"

    docker rm -f "$CNAME" 2>/dev/null || true

    docker run -d \
        --name "$CNAME" \
        --network host \
        -e BRIDGE_ID=localnet \
        -e RELAYER_ED25519_PRIVATE_KEY="$ED25519_KEY" \
        -e SVM_CONTRACT_ADDRESS="$SVM_PROGRAM_ID" \
        -e SVM_WS_URL="$SVM_WS" \
        -v "$LOCALNET_BRIDGES:/app/config/bridges.json:ro" \
        "$DOCKER_IMAGE"

    DOCKER_CONTAINERS+=("$CNAME")
    log "  Relayer $N started: container=$CNAME"
done

log "  Waiting 5s for containers to initialize..."
sleep 5

for cname in "${DOCKER_CONTAINERS[@]}"; do
    if docker inspect -f '{{.State.Running}}' "$cname" 2>/dev/null | grep -q true; then
        log "  $cname: running"
    else
        log "  $cname: NOT running!"
        docker logs "$cname" 2>&1 | tail -10
    fi
done

# ============================================================
# Step 12: Save deployment info
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
  --arg bridge_fee "$SVM_BRIDGE_FEE" \
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
    svm_admin: $svm_admin,
    svm_bridge_fee: $bridge_fee
  }' > "$DEPLOYMENT_FILE"

log ""
log "================================================================"
log "  Local E2E Environment Ready!"
log "================================================================"
log ""
log "  Solana:     RPC=$SOL_RPC  WS=$SOL_WS"
log "  1024chain:  RPC=$SVM_RPC  WS=$SVM_WS"
log ""
log "  Solana Program:  $SOL_PROGRAM_ID (fee=0)"
log "  SVM Program:     $SVM_PROGRAM_ID (fee=$SVM_BRIDGE_FEE)"
log "  Solana USDC:     $SOL_USDC_MINT"
log "  SVM USDC:        $SVM_USDC_MINT"
log ""
log "  3 Docker containers running (bridge1024-relayer-{1,2,3})"
log ""

# ============================================================
# Step 13: Run cross-chain transfer E2E tests
# ============================================================
IDL_PATH="$CONTRACT_DIR/target/idl/bridge1024.json"

E2E_ENV=(
    SOLANA_KEYPAIR_PATH="$ADMIN_SOL_KEYPAIR"
    SVM_KEYPAIR_PATH="$ADMIN_SVM_KEYPAIR"
    SOLANA_RPC_URL="$SOL_RPC"
    SVM_RPC_URL="$SVM_RPC"
    SOLANA_PROGRAM_ID="$SOL_PROGRAM_ID"
    SVM_PROGRAM_ID="$SVM_PROGRAM_ID"
    SOLANA_TOKEN_ADDRESS="$SOL_USDC_MINT"
    SVM_TOKEN_ADDRESS="$SVM_USDC_MINT"
    SOLANA_IDL_PATH="$IDL_PATH"
    SVM_IDL_PATH="$IDL_PATH"
    TEST_AMOUNT=10000
    SVM_BRIDGE_FEE="$SVM_BRIDGE_FEE"
    INITIAL_DELAY_MS=5000
    POLL_INTERVAL_MS=3000
    TIMEOUT_MS=120000
)

show_relayer_balances() {
    local label="$1"
    log ""
    log "--- Relayer Balances ($label) ---"
    for i in 0 1 2; do
        local N=$((i + 1))
        local pub="${RELAYER_PUBKEYS[$i]}"
        local sol_bal svm_bal
        sol_bal=$(solana balance "$pub" --url "$SOL_RPC" 2>/dev/null | awk '{print $1}')
        svm_bal=$(solana balance "$pub" --url "$SVM_RPC" 2>/dev/null | awk '{print $1}')
        log "  Relayer $N ($pub): Solana=${sol_bal:-?} SOL, SVM=${svm_bal:-?} SOL"
    done
}

show_relayer_balances "before E2E tests"

log ""
log "============================================"
log "  E2E Test 1: Solana -> 1024chain"
log "============================================"
if env "${E2E_ENV[@]}" npx ts-node "$DEPLOY_SCRIPTS_DIR/e2e-solana-to-svm.ts"; then
    log "PASS: Solana -> 1024chain transfer succeeded"
else
    log "FAIL: Solana -> 1024chain transfer failed"
    for cname in "${DOCKER_CONTAINERS[@]}"; do
        log "--- $cname logs (last 30 lines) ---"
        docker logs --tail 30 "$cname" 2>&1 || true
    done
    exit 1
fi

log ""
log "============================================"
log "  E2E Test 2: 1024chain -> Solana"
log "============================================"
if env "${E2E_ENV[@]}" npx ts-node "$DEPLOY_SCRIPTS_DIR/e2e-svm-to-solana.ts"; then
    log "PASS: 1024chain -> Solana transfer succeeded"
else
    log "FAIL: 1024chain -> Solana transfer failed"
    for cname in "${DOCKER_CONTAINERS[@]}"; do
        log "--- $cname logs (last 30 lines) ---"
        docker logs --tail 30 "$cname" 2>&1 || true
    done
    exit 1
fi

show_relayer_balances "after E2E tests"

log ""
log "================================================================"
log "  ALL E2E TESTS PASSED"
log "================================================================"
log ""
log "  Solana -> 1024chain: PASS (fee deducted on SVM unlock)"
log "  1024chain -> Solana: PASS (fee deducted on SVM stake)"
log ""
log "  Deployment info: $DEPLOYMENT_FILE"
log ""
