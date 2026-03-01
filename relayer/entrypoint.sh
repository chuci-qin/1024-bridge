#!/bin/bash
# ============================================
# Relayer 容器入口脚本（精简版 v2）
# ============================================
# 必需环境变量（3 个）:
#   BRIDGE_ID                    -- 桥接对标识（如 arbsep-1024test-usdc）
#   RELAYER_ECDSA_PRIVATE_KEY    -- S2E 方向的 EVM 私钥 [密]
#   RELAYER_ED25519_PRIVATE_KEY  -- E2S 方向的 Solana 私钥种子 [密]
#
# 可选环境变量:
#   EVM_CONTRACT_ADDRESS         -- 手动指定 EVM 合约地址（跳过自动获取）
#   SVM_CONTRACT_ADDRESS         -- 手动指定 SVM 程序 ID（跳过自动获取）
#   GITHUB_TOKEN                 -- GitHub PAT（仅降级到 API 获取且仓库私有时需要）
#   RELEASE_TAG                  -- GitHub Release tag（仅降级到 API 获取时使用）
# ============================================

set -e

APP_DIR="/app"
ARTIFACTS_DIR="$APP_DIR/artifacts"
BRIDGES_FILE="$APP_DIR/config/bridges.json"
GITHUB_REPO="chuci-qin/1024-bridge"

# 带时间戳的日志函数
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
# RELAYER_ECDSA_PRIVATE_KEY is only required for EVM bridges (checked later if needed)

# ============================================
# 获取合约地址（三级降级）:
#   1. 环境变量手动指定 → 直接使用
#   2. 镜像内嵌产物 → 构建时从 Release 下载，零网络请求
#   3. GitHub API 运行时获取 → 需要 GITHUB_TOKEN（私有仓库）
# ============================================
if [ -n "$EVM_CONTRACT_ADDRESS" ] && [ -n "$SVM_CONTRACT_ADDRESS" ]; then
    log "Using manually configured contract addresses"
    log "  EVM contract: $EVM_CONTRACT_ADDRESS"
    log "  SVM program:  $SVM_CONTRACT_ADDRESS"
else
    ASSET_NAME="${BRIDGE_ID}.json"
    LOCAL_ARTIFACT="$ARTIFACTS_DIR/$ASSET_NAME"
    DEPLOY_JSON=""

    # 策略 2: 读取镜像内嵌的产物文件
    if [ -f "$LOCAL_ARTIFACT" ]; then
        log "Found embedded artifact: $LOCAL_ARTIFACT"
        DEPLOY_JSON=$(cat "$LOCAL_ARTIFACT")
    fi

    # 策略 3: 降级到 GitHub API
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
            log "Fetching releases list (including pre-releases)..."
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

    EVM_CONTRACT_ADDRESS=$(echo "$DEPLOY_JSON" | jq -r '.evm.contract_address')
    SVM_CONTRACT_ADDRESS=$(echo "$DEPLOY_JSON" | jq -r '.svm.program_id')
    export EVM_CONTRACT_ADDRESS SVM_CONTRACT_ADDRESS
    log "Resolved contract addresses:"
    log "  EVM contract: $EVM_CONTRACT_ADDRESS"
    log "  SVM program:  $SVM_CONTRACT_ADDRESS"
fi

# Validate contract addresses (EVM may not be needed for Solana bridges)
# Peek ahead at bridges.json to check if this is a Solana bridge
_HAS_SOLANA=$(jq -r ".\"$BRIDGE_ID\".solana // empty" "$BRIDGES_FILE" 2>/dev/null || true)
if [ -z "$_HAS_SOLANA" ] || [ "$_HAS_SOLANA" = "null" ]; then
    # EVM bridge: both addresses required
    if [ -z "$EVM_CONTRACT_ADDRESS" ] || [ "$EVM_CONTRACT_ADDRESS" = "null" ]; then
        log_error "EVM_CONTRACT_ADDRESS is empty after resolution"
        exit 1
    fi
fi
if [ -z "$SVM_CONTRACT_ADDRESS" ] || [ "$SVM_CONTRACT_ADDRESS" = "null" ]; then
    log_error "SVM_CONTRACT_ADDRESS is empty after resolution"
    exit 1
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

# Detect bridge type: "solana" (Solana->1024chain) vs "evm" (EVM<->1024chain)
TOKEN=$(echo "$BRIDGE_CONFIG" | jq -r '.token')
SOLANA_CONFIG=$(echo "$BRIDGE_CONFIG" | jq -r '.solana // empty')
EVM_CONFIG=$(echo "$BRIDGE_CONFIG" | jq -r '.evm // empty')
SVM_CONFIG=$(echo "$BRIDGE_CONFIG" | jq -r '.svm')

if [ -n "$SOLANA_CONFIG" ] && [ "$SOLANA_CONFIG" != "null" ]; then
    BRIDGE_TYPE="solana"
    log "Detected bridge type: Solana -> 1024chain"

    # Parse Solana side (source)
    SOL_NAME=$(echo "$SOLANA_CONFIG" | jq -r '.name')
    SOL_CHAIN_ID=$(echo "$SOLANA_CONFIG" | jq -r '.chain_id')
    SOL_RPC=$(echo "$SOLANA_CONFIG" | jq -r '.rpc_url')
    SOL_PROGRAM_ID=$(echo "$SOLANA_CONFIG" | jq -r '.program_id')
    SOL_TOKEN_ADDR=$(echo "$SOLANA_CONFIG" | jq -r '.token_address')
    SOL_COMMIT=$(echo "$SOLANA_CONFIG" | jq -r '.commitment')

    # Parse SVM/1024chain side (target)
    SVM_NAME=$(echo "$SVM_CONFIG" | jq -r '.name')
    SVM_CHAIN_ID=$(echo "$SVM_CONFIG" | jq -r '.chain_id')
    SVM_RPC=$(echo "$SVM_CONFIG" | jq -r '.rpc_url')
    SVM_TOKEN_ADDR=$(echo "$SVM_CONFIG" | jq -r '.token_address')
    SVM_COMMIT=$(echo "$SVM_CONFIG" | jq -r '.commitment')

    CHAIN_MISSING=0
    for var in SOL_NAME SOL_CHAIN_ID SOL_RPC SOL_PROGRAM_ID SVM_NAME SVM_CHAIN_ID SVM_RPC; do
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

    log "Bridge: $BRIDGE_ID ($TOKEN) [Solana -> 1024chain]"
    log "  Solana: $SOL_NAME (chain_id=$SOL_CHAIN_ID)"
    log "  SVM:    $SVM_NAME (chain_id=$SVM_CHAIN_ID)"
    log "  Solana program: $SOL_PROGRAM_ID"
    log "  SVM contract:   $SVM_CONTRACT_ADDRESS"
else
    BRIDGE_TYPE="evm"
    log "Detected bridge type: EVM <-> 1024chain"

    # Parse EVM side
    EVM_NAME=$(echo "$EVM_CONFIG" | jq -r '.name')
    EVM_CHAIN_ID=$(echo "$EVM_CONFIG" | jq -r '.chain_id')
    EVM_RPC=$(echo "$EVM_CONFIG" | jq -r '.rpc_url')
    EVM_TOKEN_ADDR=$(echo "$EVM_CONFIG" | jq -r '.token_address')
    EVM_CONFIRMS=$(echo "$EVM_CONFIG" | jq -r '.confirmation_blocks')

    # Parse SVM side
    SVM_NAME=$(echo "$SVM_CONFIG" | jq -r '.name')
    SVM_CHAIN_ID=$(echo "$SVM_CONFIG" | jq -r '.chain_id')
    SVM_RPC=$(echo "$SVM_CONFIG" | jq -r '.rpc_url')
    SVM_TOKEN_ADDR=$(echo "$SVM_CONFIG" | jq -r '.token_address')
    SVM_COMMIT=$(echo "$SVM_CONFIG" | jq -r '.commitment')

    CHAIN_MISSING=0
    for var in EVM_NAME EVM_CHAIN_ID EVM_RPC SVM_NAME SVM_CHAIN_ID SVM_RPC; do
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

    log "Bridge: $BRIDGE_ID ($TOKEN)"
    log "  EVM: $EVM_NAME (chain_id=$EVM_CHAIN_ID)"
    log "  SVM: $SVM_NAME (chain_id=$SVM_CHAIN_ID)"
    log "  EVM contract: $EVM_CONTRACT_ADDRESS"
    log "  SVM contract: $SVM_CONTRACT_ADDRESS"
fi

# ============================================
# Log prefix function
# ============================================
prefix_log() {
    local name="$1"
    while IFS= read -r line; do
        printf "[%s] %s\n" "$name" "$line"
    done
}

if [ "$BRIDGE_TYPE" = "solana" ]; then
    # ==============================================
    # Solana -> 1024chain bridge: sol2svm-listener + sol2svm-submitter
    # ==============================================

    mkdir -p "$APP_DIR/sol2svm-listener" "$APP_DIR/sol2svm-submitter"

    # sol2svm-listener .env (source=Solana, target=1024chain)
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
    log "Generated sol2svm-listener/.env"

    # sol2svm-submitter .env (source=Solana, target=1024chain)
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
    log "Generated sol2svm-submitter/.env"

    log "===== Configuration Summary ====="
    for envfile in "$APP_DIR/sol2svm-listener/.env" "$APP_DIR/sol2svm-submitter/.env"; do
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

    mkdir -p "$APP_DIR/sol2svm-listener/.relayer/queue"

    SOL2SVM_LISTENER_BIN="$APP_DIR/sol2svm-listener/sol2svm-listener"
    SOL2SVM_SUBMITTER_BIN="$APP_DIR/sol2svm-submitter/sol2svm-submitter"

    for bin in "$SOL2SVM_LISTENER_BIN" "$SOL2SVM_SUBMITTER_BIN"; do
        if [ ! -f "$bin" ]; then
            log_error "Binary not found: $bin"
            exit 1
        fi
    done
    log "All binaries found"

    log "Starting sol2svm components..."

    (cd "$APP_DIR/sol2svm-listener" && set -a && . ./.env && set +a && exec "$SOL2SVM_LISTENER_BIN" 2>&1) | prefix_log "sol2svm-listener" &
    PIPE_LISTENER_PID=$!
    log "Started sol2svm-listener (PIPE_PID=$PIPE_LISTENER_PID)"

    (cd "$APP_DIR/sol2svm-submitter" && set -a && . ./.env && set +a && exec "$SOL2SVM_SUBMITTER_BIN" 2>&1) | prefix_log "sol2svm-submitter" &
    PIPE_SUBMITTER_PID=$!
    log "Started sol2svm-submitter (PIPE_PID=$PIPE_SUBMITTER_PID)"

    log "All sol2svm components started. Monitoring..."

    wait -n $PIPE_LISTENER_PID $PIPE_SUBMITTER_PID
    EXIT_CODE=$?
    log_error "A component pipeline exited with code $EXIT_CODE"

    for pid_name in "sol2svm-listener:$PIPE_LISTENER_PID" "sol2svm-submitter:$PIPE_SUBMITTER_PID"; do
        name="${pid_name%%:*}"
        pid="${pid_name##*:}"
        if ! kill -0 "$pid" 2>/dev/null; then
            log_error "  $name (PID=$pid) has exited"
        else
            log "  $name (PID=$pid) still running, sending SIGTERM"
            kill "$pid" 2>/dev/null || true
        fi
    done

else
    # ==============================================
    # EVM <-> 1024chain bridge: s2e + e2s-listener + e2s-submitter
    # ==============================================

    # Generate S2E .env (source=SVM, target=EVM)
    cat > "$APP_DIR/s2e/.env" <<ENVEOF
SERVICE__NAME="s2e"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$SVM_NAME"
SOURCE_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$SVM_RPC"
SOURCE_CHAIN__WS_URL="$(echo "$SVM_RPC" | sed 's|^https://|wss://|; s|^http://|ws://|')"
SOURCE_CHAIN__CONTRACT_ADDRESS="$SVM_CONTRACT_ADDRESS"
SOURCE_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__NAME="$EVM_NAME"
TARGET_CHAIN__CHAIN_ID="$EVM_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$EVM_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$EVM_CONTRACT_ADDRESS"
TARGET_CHAIN__CONFIRMATION_BLOCKS="$EVM_CONFIRMS"
TARGET_CHAIN__USDC_MINT="$EVM_TOKEN_ADDR"
RELAYER__ECDSA_PRIVATE_KEY="$RELAYER_ECDSA_PRIVATE_KEY"
API__PORT="8081"
LOGGING__LEVEL="info"
LOGGING__FORMAT="json"
ENVEOF
    log "Generated s2e/.env"

    # Generate E2S Listener .env (source=EVM, target=SVM)
    cat > "$APP_DIR/e2s-listener/.env" <<ENVEOF
SERVICE__NAME="e2s-listener"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$EVM_NAME"
SOURCE_CHAIN__CHAIN_ID="$EVM_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$EVM_RPC"
SOURCE_CHAIN__CONTRACT_ADDRESS="$EVM_CONTRACT_ADDRESS"
SOURCE_CHAIN__CONFIRMATION_BLOCKS="$EVM_CONFIRMS"
TARGET_CHAIN__NAME="$SVM_NAME"
TARGET_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$SVM_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$SVM_CONTRACT_ADDRESS"
TARGET_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__USDC_MINT="$SVM_TOKEN_ADDR"
API__PORT="8083"
LOGGING__LEVEL="info"
LOGGING__FORMAT="text"
ENVEOF
    log "Generated e2s-listener/.env"

    # Generate E2S Submitter .env (source=EVM, target=SVM)
    cat > "$APP_DIR/e2s-submitter/.env" <<ENVEOF
SERVICE__NAME="e2s-submitter"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$EVM_NAME"
SOURCE_CHAIN__CHAIN_ID="$EVM_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$EVM_RPC"
SOURCE_CHAIN__CONTRACT_ADDRESS="$EVM_CONTRACT_ADDRESS"
SOURCE_CHAIN__CONFIRMATION_BLOCKS="$EVM_CONFIRMS"
TARGET_CHAIN__NAME="$SVM_NAME"
TARGET_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$SVM_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$SVM_CONTRACT_ADDRESS"
TARGET_CHAIN__COMMITMENT="$SVM_COMMIT"
TARGET_CHAIN__USDC_MINT="$SVM_TOKEN_ADDR"
RELAYER__ED25519_PRIVATE_KEY="$RELAYER_ED25519_PRIVATE_KEY"
QUEUE__PATH="$APP_DIR/e2s-listener/.relayer/queue"
API__PORT="8082"
LOGGING__LEVEL="info"
LOGGING__FORMAT="text"
ENVEOF
    log "Generated e2s-submitter/.env"

    log "===== Configuration Summary ====="
    for envfile in "$APP_DIR/s2e/.env" "$APP_DIR/e2s-listener/.env" "$APP_DIR/e2s-submitter/.env"; do
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

    mkdir -p "$APP_DIR/e2s-listener/.relayer/queue"

    S2E_BIN="$APP_DIR/s2e/s2e-relayer"
    E2S_LISTENER_BIN="$APP_DIR/e2s-listener/e2s-listener"
    E2S_SUBMITTER_BIN="$APP_DIR/e2s-submitter/e2s-submitter"

    for bin in "$S2E_BIN" "$E2S_LISTENER_BIN" "$E2S_SUBMITTER_BIN"; do
        if [ ! -f "$bin" ]; then
            log_error "Binary not found: $bin"
            exit 1
        fi
    done
    log "All binaries found"

    log "Starting EVM<->SVM components..."

    (cd "$APP_DIR/s2e" && set -a && . ./.env && set +a && exec "$S2E_BIN" 2>&1) | prefix_log "s2e" &
    PIPE_S2E_PID=$!
    log "Started s2e (PIPE_PID=$PIPE_S2E_PID)"

    (cd "$APP_DIR/e2s-listener" && set -a && . ./.env && set +a && exec "$E2S_LISTENER_BIN" 2>&1) | prefix_log "e2s-listener" &
    PIPE_LISTENER_PID=$!
    log "Started e2s-listener (PIPE_PID=$PIPE_LISTENER_PID)"

    (cd "$APP_DIR/e2s-submitter" && set -a && . ./.env && set +a && exec "$E2S_SUBMITTER_BIN" 2>&1) | prefix_log "e2s-submitter" &
    PIPE_SUBMITTER_PID=$!
    log "Started e2s-submitter (PIPE_PID=$PIPE_SUBMITTER_PID)"

    log "All components started. Monitoring..."

    wait -n $PIPE_S2E_PID $PIPE_LISTENER_PID $PIPE_SUBMITTER_PID
    EXIT_CODE=$?
    log_error "A component pipeline exited with code $EXIT_CODE"

    for pid_name in "s2e:$PIPE_S2E_PID" "e2s-listener:$PIPE_LISTENER_PID" "e2s-submitter:$PIPE_SUBMITTER_PID"; do
        name="${pid_name%%:*}"
        pid="${pid_name##*:}"
        if ! kill -0 "$pid" 2>/dev/null; then
            log_error "  $name (PID=$pid) has exited"
        else
            log "  $name (PID=$pid) still running, sending SIGTERM"
            kill "$pid" 2>/dev/null || true
        fi
    done
fi

sleep 2
log "Container exiting (code=$EXIT_CODE)"
exit "$EXIT_CODE"
