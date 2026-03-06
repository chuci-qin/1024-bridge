#!/bin/bash
# ============================================
# Relayer 容器入口脚本 — Sol-1024 桥专用
# ============================================
# 必需环境变量（2 个）:
#   BRIDGE_ID                    -- 桥接对标识（如 soldev-1024test-usdc）
#   RELAYER_ED25519_PRIVATE_KEY  -- Ed25519 私钥种子 [密]
#
# 可选环境变量:
#   SVM_CONTRACT_ADDRESS         -- 手动指定 SVM 程序 ID（跳过自动获取）
#   SOL_PROGRAM_ID               -- 手动指定 Solana 程序 ID（不设则等于 SVM_CONTRACT_ADDRESS）
#   GITHUB_TOKEN                 -- GitHub PAT（仅降级到 API 获取且仓库私有时需要）
#   RELEASE_TAG                  -- GitHub Release tag（仅降级到 API 获取时使用）
# ============================================

set -e

APP_DIR="/app"
ARTIFACTS_DIR="$APP_DIR/artifacts"
BRIDGES_FILE="$APP_DIR/config/bridges.json"
GITHUB_REPO="chuci-qin/1024-bridge"

log() {
    echo "[entrypoint][$(date '+%Y-%m-%d %H:%M:%S')] $*"
}

log_error() {
    echo "[entrypoint][$(date '+%Y-%m-%d %H:%M:%S')] ERROR: $*" >&2
}

# ============================================
# Validate required environment variables
# ============================================
if [ -z "$BRIDGE_ID" ]; then
    log_error "BRIDGE_ID is not set"
    exit 1
fi
if [ -z "$RELAYER_ED25519_PRIVATE_KEY" ]; then
    log_error "RELAYER_ED25519_PRIVATE_KEY is not set"
    exit 1
fi

# ============================================
# 获取合约地址（统一合约两边 program_id 相同）
# ============================================
if [ -n "$SVM_CONTRACT_ADDRESS" ]; then
    log "Using manually configured SVM contract address: $SVM_CONTRACT_ADDRESS"
    SOL_PROGRAM_ID="${SOL_PROGRAM_ID:-$SVM_CONTRACT_ADDRESS}"
    log "Using SOL_PROGRAM_ID: $SOL_PROGRAM_ID"
else
    ASSET_NAME="${BRIDGE_ID}.json"
    LOCAL_ARTIFACT="$ARTIFACTS_DIR/$ASSET_NAME"
    DEPLOY_JSON=""

    if [ -f "$LOCAL_ARTIFACT" ]; then
        log "Found embedded artifact: $LOCAL_ARTIFACT"
        DEPLOY_JSON=$(cat "$LOCAL_ARTIFACT")
    fi

    if [ -z "$DEPLOY_JSON" ]; then
        TAG="${RELEASE_TAG:-latest}"
        log "No embedded artifact found, falling back to GitHub API..."

        CURL_AUTH_ARGS=()
        if [ -n "$GITHUB_TOKEN" ]; then
            CURL_AUTH_ARGS=(-H "Authorization: token $GITHUB_TOKEN")
            log "Using GITHUB_TOKEN for API authentication"
        else
            log "WARNING: GITHUB_TOKEN not set. Private repos will fail."
        fi

        if [ "$TAG" = "latest" ]; then
            RELEASE_URL="https://api.github.com/repos/$GITHUB_REPO/releases"
            log "Fetching releases list..."
            RELEASES_JSON=$(curl -sL --fail "${CURL_AUTH_ARGS[@]}" "$RELEASE_URL?per_page=10" 2>/dev/null) || {
                log_error "Failed to fetch releases from $RELEASE_URL"
                exit 1
            }
            RELEASE_JSON=$(echo "$RELEASES_JSON" | jq -c "[.[] | select(.assets[] | .name == \"$ASSET_NAME\")] | first // empty")
            if [ -z "$RELEASE_JSON" ] || [ "$RELEASE_JSON" = "null" ]; then
                ALL_TAGS=$(echo "$RELEASES_JSON" | jq -r '.[].tag_name' | tr '\n' ', ')
                log_error "No release found with asset '$ASSET_NAME'. Recent tags: $ALL_TAGS"
                exit 1
            fi
        else
            RELEASE_URL="https://api.github.com/repos/$GITHUB_REPO/releases/tags/$TAG"
            log "Fetching release $TAG..."
            RELEASE_JSON=$(curl -sL --fail "${CURL_AUTH_ARGS[@]}" "$RELEASE_URL" 2>/dev/null) || {
                log_error "Failed to fetch release info from $RELEASE_URL"
                exit 1
            }
        fi

        if [ -n "$GITHUB_TOKEN" ]; then
            ASSET_URL=$(echo "$RELEASE_JSON" | jq -r ".assets[] | select(.name==\"$ASSET_NAME\") | .url")
        else
            ASSET_URL=$(echo "$RELEASE_JSON" | jq -r ".assets[] | select(.name==\"$ASSET_NAME\") | .browser_download_url")
        fi
        if [ -z "$ASSET_URL" ] || [ "$ASSET_URL" = "null" ]; then
            AVAILABLE=$(echo "$RELEASE_JSON" | jq -r '.assets[].name' | tr '\n' ', ')
            log_error "Asset '$ASSET_NAME' not found. Available: $AVAILABLE"
            exit 1
        fi

        DEPLOY_JSON=$(curl -sL --fail "${CURL_AUTH_ARGS[@]}" -H "Accept: application/octet-stream" "$ASSET_URL" 2>/dev/null) || {
            log_error "Failed to download deployment artifact from $ASSET_URL"
            exit 1
        }
        RELEASE_TAG_ACTUAL=$(echo "$RELEASE_JSON" | jq -r '.tag_name')
        log "Fetched from release $RELEASE_TAG_ACTUAL"
    fi

    SVM_CONTRACT_ADDRESS=$(echo "$DEPLOY_JSON" | jq -r '.svm.program_id')
    SOL_PROGRAM_ID=$(echo "$DEPLOY_JSON" | jq -r '.solana.program_id // .program_id')
    export SVM_CONTRACT_ADDRESS SOL_PROGRAM_ID
    log "Resolved contract addresses from deployment artifact:"
    log "  Solana program: $SOL_PROGRAM_ID"
    log "  SVM contract:   $SVM_CONTRACT_ADDRESS"
fi

if [ -z "$SVM_CONTRACT_ADDRESS" ] || [ "$SVM_CONTRACT_ADDRESS" = "null" ]; then
    log_error "SVM_CONTRACT_ADDRESS is empty after resolution"
    exit 1
fi
if [ -z "$SOL_PROGRAM_ID" ] || [ "$SOL_PROGRAM_ID" = "null" ]; then
    SOL_PROGRAM_ID="$SVM_CONTRACT_ADDRESS"
    log "SOL_PROGRAM_ID not in artifact, using unified program_id: $SOL_PROGRAM_ID"
fi

# ============================================
# 校验 bridges.json 存在
# ============================================
if [ ! -f "$BRIDGES_FILE" ]; then
    log_error "Bridges config not found: $BRIDGES_FILE"
    exit 1
fi

# ============================================
# 从 bridges.json 读取链配置
# ============================================
BRIDGE_CONFIG=$(jq -r ".\"$BRIDGE_ID\"" "$BRIDGES_FILE")
if [ "$BRIDGE_CONFIG" = "null" ] || [ -z "$BRIDGE_CONFIG" ]; then
    AVAILABLE=$(jq -r 'keys | join(", ")' "$BRIDGES_FILE")
    log_error "Unknown BRIDGE_ID=$BRIDGE_ID. Available: $AVAILABLE"
    exit 1
fi

TOKEN=$(echo "$BRIDGE_CONFIG" | jq -r '.token')
SOLANA_CONFIG=$(echo "$BRIDGE_CONFIG" | jq -r '.solana')
SVM_CONFIG=$(echo "$BRIDGE_CONFIG" | jq -r '.svm')

if [ "$SOLANA_CONFIG" = "null" ] || [ -z "$SOLANA_CONFIG" ]; then
    log_error "Bridge $BRIDGE_ID does not have a 'solana' config section"
    exit 1
fi

SOL_NAME=$(echo "$SOLANA_CONFIG" | jq -r '.name')
SOL_CHAIN_ID=$(echo "$SOLANA_CONFIG" | jq -r '.chain_id')
SOL_RPC=$(echo "$SOLANA_CONFIG" | jq -r '.rpc_url')
SOL_TOKEN_ADDR=$(echo "$SOLANA_CONFIG" | jq -r '.token_address')
SOL_COMMIT=$(echo "$SOLANA_CONFIG" | jq -r '.commitment')

SVM_NAME=$(echo "$SVM_CONFIG" | jq -r '.name')
SVM_CHAIN_ID=$(echo "$SVM_CONFIG" | jq -r '.chain_id')
SVM_RPC=$(echo "$SVM_CONFIG" | jq -r '.rpc_url')
SVM_TOKEN_ADDR=$(echo "$SVM_CONFIG" | jq -r '.token_address')
SVM_COMMIT=$(echo "$SVM_CONFIG" | jq -r '.commitment')

CHAIN_MISSING=0
for var in SOL_NAME SOL_CHAIN_ID SOL_RPC SVM_NAME SVM_CHAIN_ID SVM_RPC; do
    val="${!var}"
    if [ -z "$val" ] || [ "$val" = "null" ]; then
        log_error "Bridge config field $var is empty or null"
        CHAIN_MISSING=1
    fi
done
if [ "$CHAIN_MISSING" -eq 1 ]; then
    log_error "bridges.json for '$BRIDGE_ID' has missing/null fields. Check your config."
    exit 1
fi

log "Bridge: $BRIDGE_ID ($TOKEN) [Solana <-> 1024chain]"
log "  Solana: $SOL_NAME (chain_id=$SOL_CHAIN_ID)"
log "  SVM:    $SVM_NAME (chain_id=$SVM_CHAIN_ID)"
log "  Solana program: $SOL_PROGRAM_ID"
log "  SVM contract:   $SVM_CONTRACT_ADDRESS"

# ============================================
# Log prefix function
# ============================================
prefix_log() {
    local name="$1"
    while IFS= read -r line; do
        printf "[%s] %s\n" "$name" "$line"
    done
}

# ============================================
# Generate .env files
# ============================================

mkdir -p "$APP_DIR/sol2svm-listener" "$APP_DIR/sol2svm-submitter"
mkdir -p "$APP_DIR/svm2sol-listener" "$APP_DIR/svm2sol-submitter"

# sol2svm-listener
cat > "$APP_DIR/sol2svm-listener/.env" <<ENVEOF
SERVICE__NAME="sol2svm-listener"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$SOL_NAME"
SOURCE_CHAIN__CHAIN_ID="$SOL_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$SOL_RPC"
SOURCE_CHAIN__CONTRACT_ADDRESS="$SOL_PROGRAM_ID"
SOURCE_CHAIN__COMMITMENT="$SOL_COMMIT"
TARGET_CHAIN__NAME="$SVM_NAME"
TARGET_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$SVM_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$SVM_CONTRACT_ADDRESS"
TARGET_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__USDC_MINT="$SVM_TOKEN_ADDR"
QUEUE__PATH="$APP_DIR/sol2svm-listener/.relayer/queue"
API__PORT="8085"
LOGGING__LEVEL="info"
LOGGING__FORMAT="text"
ENVEOF

# sol2svm-submitter
cat > "$APP_DIR/sol2svm-submitter/.env" <<ENVEOF
SERVICE__NAME="sol2svm-submitter"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$SOL_NAME"
SOURCE_CHAIN__CHAIN_ID="$SOL_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$SOL_RPC"
SOURCE_CHAIN__CONTRACT_ADDRESS="$SOL_PROGRAM_ID"
SOURCE_CHAIN__COMMITMENT="$SOL_COMMIT"
TARGET_CHAIN__NAME="$SVM_NAME"
TARGET_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$SVM_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$SVM_CONTRACT_ADDRESS"
TARGET_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__USDC_MINT="$SVM_TOKEN_ADDR"
RELAYER__ED25519_PRIVATE_KEY="$RELAYER_ED25519_PRIVATE_KEY"
QUEUE__PATH="$APP_DIR/sol2svm-listener/.relayer/queue"
API__PORT="8084"
LOGGING__LEVEL="info"
LOGGING__FORMAT="text"
ENVEOF

# svm2sol-listener
SVM_WS_URL="${SVM_WS_URL:-$(echo "$SVM_RPC" | sed 's|^https://|wss://|; s|^http://|ws://|')}"
cat > "$APP_DIR/svm2sol-listener/.env" <<ENVEOF
SERVICE__NAME="svm2sol-listener"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$SVM_NAME"
SOURCE_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$SVM_RPC"
SOURCE_CHAIN__WS_URL="$SVM_WS_URL"
SOURCE_CHAIN__CONTRACT_ADDRESS="$SVM_CONTRACT_ADDRESS"
SOURCE_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__NAME="$SOL_NAME"
TARGET_CHAIN__CHAIN_ID="$SOL_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$SOL_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$SOL_PROGRAM_ID"
TARGET_CHAIN__COMMITMENT="$SOL_COMMIT"
TARGET_CHAIN__USDC_MINT="$SOL_TOKEN_ADDR"
QUEUE__PATH="$APP_DIR/svm2sol-listener/.relayer/queue"
API__PORT="8087"
LOGGING__LEVEL="info"
LOGGING__FORMAT="text"
ENVEOF

# svm2sol-submitter
cat > "$APP_DIR/svm2sol-submitter/.env" <<ENVEOF
SERVICE__NAME="svm2sol-submitter"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$SVM_NAME"
SOURCE_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$SVM_RPC"
SOURCE_CHAIN__CONTRACT_ADDRESS="$SVM_CONTRACT_ADDRESS"
SOURCE_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__NAME="$SOL_NAME"
TARGET_CHAIN__CHAIN_ID="$SOL_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$SOL_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$SOL_PROGRAM_ID"
TARGET_CHAIN__COMMITMENT="$SOL_COMMIT"
TARGET_CHAIN__USDC_MINT="$SOL_TOKEN_ADDR"
RELAYER__ED25519_PRIVATE_KEY="$RELAYER_ED25519_PRIVATE_KEY"
QUEUE__PATH="$APP_DIR/svm2sol-listener/.relayer/queue"
API__PORT="8086"
LOGGING__LEVEL="info"
LOGGING__FORMAT="text"
ENVEOF

log "Generated all .env files"

# ============================================
# Print config summary (mask secrets)
# ============================================
log "===== Configuration Summary ====="
for envfile in "$APP_DIR/sol2svm-listener/.env" "$APP_DIR/sol2svm-submitter/.env" "$APP_DIR/svm2sol-listener/.env" "$APP_DIR/svm2sol-submitter/.env"; do
    component=$(basename "$(dirname "$envfile")")
    log "--- $component ---"
    while IFS= read -r line; do
        key="${line%%=*}"
        if echo "$key" | grep -iqE 'key|secret|password|private|mnemonic'; then
            log "  $key=****MASKED****"
        else
            log "  $line"
        fi
    done < "$envfile"
done
log "================================="

# ============================================
# Create queue directories
# ============================================
mkdir -p "$APP_DIR/sol2svm-listener/.relayer/queue"
mkdir -p "$APP_DIR/svm2sol-listener/.relayer/queue"

# ============================================
# Verify binaries exist
# ============================================
SOL2SVM_LISTENER_BIN="$APP_DIR/sol2svm-listener/sol2svm-listener"
SOL2SVM_SUBMITTER_BIN="$APP_DIR/sol2svm-submitter/sol2svm-submitter"
SVM2SOL_LISTENER_BIN="$APP_DIR/svm2sol-listener/svm2sol-listener"
SVM2SOL_SUBMITTER_BIN="$APP_DIR/svm2sol-submitter/svm2sol-submitter"

for bin in "$SOL2SVM_LISTENER_BIN" "$SOL2SVM_SUBMITTER_BIN" "$SVM2SOL_LISTENER_BIN" "$SVM2SOL_SUBMITTER_BIN"; do
    if [ ! -f "$bin" ]; then
        log_error "Binary not found: $bin"
        exit 1
    fi
done
log "All binaries found"

# ============================================
# Start all components
# ============================================
log "Starting Solana <-> 1024chain components..."

(cd "$APP_DIR/sol2svm-listener" && set -a && . ./.env && set +a && exec "$SOL2SVM_LISTENER_BIN" 2>&1) | prefix_log "sol2svm-listener" &
PIPE_SOL2SVM_LISTENER_PID=$!
log "Started sol2svm-listener (PIPE_PID=$PIPE_SOL2SVM_LISTENER_PID)"

(cd "$APP_DIR/sol2svm-submitter" && set -a && . ./.env && set +a && exec "$SOL2SVM_SUBMITTER_BIN" 2>&1) | prefix_log "sol2svm-submitter" &
PIPE_SOL2SVM_SUBMITTER_PID=$!
log "Started sol2svm-submitter (PIPE_PID=$PIPE_SOL2SVM_SUBMITTER_PID)"

(cd "$APP_DIR/svm2sol-listener" && set -a && . ./.env && set +a && exec "$SVM2SOL_LISTENER_BIN" 2>&1) | prefix_log "svm2sol-listener" &
PIPE_SVM2SOL_LISTENER_PID=$!
log "Started svm2sol-listener (PIPE_PID=$PIPE_SVM2SOL_LISTENER_PID)"

(cd "$APP_DIR/svm2sol-submitter" && set -a && . ./.env && set +a && exec "$SVM2SOL_SUBMITTER_BIN" 2>&1) | prefix_log "svm2sol-submitter" &
PIPE_SVM2SOL_SUBMITTER_PID=$!
log "Started svm2sol-submitter (PIPE_PID=$PIPE_SVM2SOL_SUBMITTER_PID)"

log "All bidirectional Solana components started. Monitoring..."

wait -n $PIPE_SOL2SVM_LISTENER_PID $PIPE_SOL2SVM_SUBMITTER_PID $PIPE_SVM2SOL_LISTENER_PID $PIPE_SVM2SOL_SUBMITTER_PID
EXIT_CODE=$?
log_error "A component pipeline exited with code $EXIT_CODE"

for pid_name in "sol2svm-listener:$PIPE_SOL2SVM_LISTENER_PID" "sol2svm-submitter:$PIPE_SOL2SVM_SUBMITTER_PID" "svm2sol-listener:$PIPE_SVM2SOL_LISTENER_PID" "svm2sol-submitter:$PIPE_SVM2SOL_SUBMITTER_PID"; do
    name="${pid_name%%:*}"
    pid="${pid_name##*:}"
    if ! kill -0 "$pid" 2>/dev/null; then
        log_error "  $name (PID=$pid) has exited"
    else
        log "  $name (PID=$pid) still running, sending SIGTERM"
        kill "$pid" 2>/dev/null || true
    fi
done

sleep 2
log "Container exiting (code=$EXIT_CODE)"
exit "$EXIT_CODE"
