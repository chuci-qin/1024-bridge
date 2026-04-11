#!/bin/bash
# ============================================
# Relayer 容器入口脚本（v3 — 4 组件 + /data 持久化）
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
#   RPC_OVERRIDE_{CHAIN_NAME}    -- 按链名覆盖 RPC URL（如 RPC_OVERRIDE_ARBITRUM_SEPOLIA, RPC_OVERRIDE_1024CHAIN_TESTNET）
#                                   链名来自 bridges.json 的 name 字段，空格/横杠转下划线并转大写
# ============================================

set -e

APP_DIR="/app"
DATA_DIR="/data"
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
# 创建持久化数据目录
# ============================================
mkdir -p "$DATA_DIR/s2e/queue" "$DATA_DIR/e2s/queue" "$DATA_DIR/logs"
log "Data directories initialized under $DATA_DIR"

# ============================================
# 校验必需的环境变量
# ============================================
MISSING=0
for var in BRIDGE_ID RELAYER_ECDSA_PRIVATE_KEY RELAYER_ED25519_PRIVATE_KEY; do
    if [ -z "${!var}" ]; then
        log_error "$var is not set"
        MISSING=1
    fi
done
if [ "$MISSING" -eq 1 ]; then
    log_error "Required: BRIDGE_ID, RELAYER_ECDSA_PRIVATE_KEY, RELAYER_ED25519_PRIVATE_KEY"
    exit 1
fi

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

# 校验合约地址已获取（无论手动还是自动）
if [ -z "$EVM_CONTRACT_ADDRESS" ] || [ "$EVM_CONTRACT_ADDRESS" = "null" ]; then
    log_error "EVM_CONTRACT_ADDRESS is empty after resolution"
    exit 1
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

# 解析桥接对信息
TOKEN=$(echo "$BRIDGE_CONFIG" | jq -r '.token')
EVM_CONFIG=$(echo "$BRIDGE_CONFIG" | jq -r '.evm')
SVM_CONFIG=$(echo "$BRIDGE_CONFIG" | jq -r '.svm')

# 解析 EVM 侧
EVM_NAME=$(echo "$EVM_CONFIG" | jq -r '.name')
EVM_CHAIN_ID=$(echo "$EVM_CONFIG" | jq -r '.chain_id')
EVM_RPC=$(echo "$EVM_CONFIG" | jq -r '.rpc_url')
EVM_TOKEN_ADDR=$(echo "$EVM_CONFIG" | jq -r '.token_address')
EVM_CONFIRMS=$(echo "$EVM_CONFIG" | jq -r '.confirmation_blocks')

# 解析 SVM 侧
SVM_NAME=$(echo "$SVM_CONFIG" | jq -r '.name')
SVM_CHAIN_ID=$(echo "$SVM_CONFIG" | jq -r '.chain_id')
SVM_RPC=$(echo "$SVM_CONFIG" | jq -r '.rpc_url')
SVM_TOKEN_ADDR=$(echo "$SVM_CONFIG" | jq -r '.token_address')
SVM_COMMIT=$(echo "$SVM_CONFIG" | jq -r '.commitment')

# 校验解析出的链配置值非空
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

# ============================================
# RPC Override: check RPC_OVERRIDE_{CHAIN_NAME}
# ============================================
to_env_key() {
    echo "$1" | tr '[:lower:]' '[:upper:]' | sed 's/[[:space:]-]/_/g'
}

EVM_RPC_OVERRIDE_VAR="RPC_OVERRIDE_$(to_env_key "$EVM_NAME")"
SVM_RPC_OVERRIDE_VAR="RPC_OVERRIDE_$(to_env_key "$SVM_NAME")"

if [ -n "${!EVM_RPC_OVERRIDE_VAR}" ]; then
    log "RPC override ($EVM_RPC_OVERRIDE_VAR): ${EVM_RPC} -> ${!EVM_RPC_OVERRIDE_VAR}"
    EVM_RPC="${!EVM_RPC_OVERRIDE_VAR}"
fi
if [ -n "${!SVM_RPC_OVERRIDE_VAR}" ]; then
    log "RPC override ($SVM_RPC_OVERRIDE_VAR): ${SVM_RPC} -> ${!SVM_RPC_OVERRIDE_VAR}"
    SVM_RPC="${!SVM_RPC_OVERRIDE_VAR}"
fi

log "Bridge: $BRIDGE_ID ($TOKEN)"
log "  EVM: $EVM_NAME (chain_id=$EVM_CHAIN_ID)"
log "  SVM: $SVM_NAME (chain_id=$SVM_CHAIN_ID)"
log "  EVM contract: $EVM_CONTRACT_ADDRESS"
log "  SVM contract: $SVM_CONTRACT_ADDRESS"
log "  EVM RPC: $EVM_RPC"
log "  SVM RPC: $SVM_RPC"

# ============================================
# 生成 S2E Listener .env (source=SVM, target=EVM)
# ============================================
cat > "$APP_DIR/s2e-listener.env" <<ENVEOF
SERVICE__NAME="s2e-listener"
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
QUEUE__PATH="$DATA_DIR/s2e/queue"
API__PORT="8081"
LOGGING__LEVEL="info"
LOGGING__FORMAT="json"
LOGGING__LOG_FILE="$DATA_DIR/logs/s2e-listener.log"
ENVEOF
log "Generated s2e-listener.env"

# ============================================
# 生成 S2E Submitter .env (source=SVM, target=EVM)
# ============================================
cat > "$APP_DIR/s2e-submitter.env" <<ENVEOF
SERVICE__NAME="s2e-submitter"
SERVICE__VERSION="0.1.0"
SERVICE__WORKER_POOL_SIZE="5"
SOURCE_CHAIN__NAME="$SVM_NAME"
SOURCE_CHAIN__CHAIN_ID="$SVM_CHAIN_ID"
SOURCE_CHAIN__RPC_URL="$SVM_RPC"
SOURCE_CHAIN__CONTRACT_ADDRESS="$SVM_CONTRACT_ADDRESS"
TARGET_CHAIN__NAME="$EVM_NAME"
TARGET_CHAIN__CHAIN_ID="$EVM_CHAIN_ID"
TARGET_CHAIN__RPC_URL="$EVM_RPC"
TARGET_CHAIN__CONTRACT_ADDRESS="$EVM_CONTRACT_ADDRESS"
TARGET_CHAIN__CONFIRMATION_BLOCKS="$EVM_CONFIRMS"
RELAYER__ECDSA_PRIVATE_KEY="$RELAYER_ECDSA_PRIVATE_KEY"
QUEUE__PATH="$DATA_DIR/s2e/queue"
API__PORT="8084"
LOGGING__LEVEL="info"
LOGGING__FORMAT="json"
LOGGING__LOG_FILE="$DATA_DIR/logs/s2e-submitter.log"
ENVEOF
log "Generated s2e-submitter.env"

# ============================================
# 生成 E2S Listener .env (source=EVM, target=SVM)
# ============================================
cat > "$APP_DIR/e2s-listener.env" <<ENVEOF
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
QUEUE__PATH="$DATA_DIR/e2s/queue"
API__PORT="8083"
LOGGING__LEVEL="info"
LOGGING__FORMAT="json"
LOGGING__LOG_FILE="$DATA_DIR/logs/e2s-listener.log"
ENVEOF
log "Generated e2s-listener.env"

# ============================================
# 生成 E2S Submitter .env (source=EVM, target=SVM)
# ============================================
cat > "$APP_DIR/e2s-submitter.env" <<ENVEOF
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
QUEUE__PATH="$DATA_DIR/e2s/queue"
API__PORT="8082"
LOGGING__LEVEL="info"
LOGGING__FORMAT="json"
LOGGING__LOG_FILE="$DATA_DIR/logs/e2s-submitter.log"
ENVEOF
log "Generated e2s-submitter.env"

# ============================================
# 打印配置摘要（敏感值脱敏）
# ============================================
log "===== Configuration Summary ====="
for envfile in "$APP_DIR/s2e-listener.env" "$APP_DIR/s2e-submitter.env" "$APP_DIR/e2s-listener.env" "$APP_DIR/e2s-submitter.env"; do
    component=$(basename "$envfile" .env)
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
# 检查二进制文件
# ============================================
S2E_LISTENER_BIN="$APP_DIR/s2e-listener"
S2E_SUBMITTER_BIN="$APP_DIR/s2e-submitter"
E2S_LISTENER_BIN="$APP_DIR/e2s-listener"
E2S_SUBMITTER_BIN="$APP_DIR/e2s-submitter"

for bin in "$S2E_LISTENER_BIN" "$S2E_SUBMITTER_BIN" "$E2S_LISTENER_BIN" "$E2S_SUBMITTER_BIN"; do
    if [ ! -f "$bin" ]; then
        log_error "Binary not found: $bin"
        exit 1
    fi
done
log "All binaries found"

# ============================================
# 日志前缀函数
# ============================================
prefix_log() {
    local name="$1"
    while IFS= read -r line; do
        printf "[%s] %s\n" "$name" "$line"
    done
}

# ============================================
# 启动四个组件
# ============================================
log "Starting components..."

(set -a && . "$APP_DIR/s2e-listener.env" && set +a && exec "$S2E_LISTENER_BIN" 2>&1) | prefix_log "s2e-listener" &
PIPE_S2E_LISTENER_PID=$!
log "Started s2e-listener (PIPE_PID=$PIPE_S2E_LISTENER_PID)"

(set -a && . "$APP_DIR/s2e-submitter.env" && set +a && exec "$S2E_SUBMITTER_BIN" 2>&1) | prefix_log "s2e-submitter" &
PIPE_S2E_SUBMITTER_PID=$!
log "Started s2e-submitter (PIPE_PID=$PIPE_S2E_SUBMITTER_PID)"

(set -a && . "$APP_DIR/e2s-listener.env" && set +a && exec "$E2S_LISTENER_BIN" 2>&1) | prefix_log "e2s-listener" &
PIPE_E2S_LISTENER_PID=$!
log "Started e2s-listener (PIPE_PID=$PIPE_E2S_LISTENER_PID)"

(set -a && . "$APP_DIR/e2s-submitter.env" && set +a && exec "$E2S_SUBMITTER_BIN" 2>&1) | prefix_log "e2s-submitter" &
PIPE_E2S_SUBMITTER_PID=$!
log "Started e2s-submitter (PIPE_PID=$PIPE_E2S_SUBMITTER_PID)"

log "All components started. Monitoring..."

# ============================================
# 监控子进程，任一退出则容器退出
# ============================================
wait -n $PIPE_S2E_LISTENER_PID $PIPE_S2E_SUBMITTER_PID $PIPE_E2S_LISTENER_PID $PIPE_E2S_SUBMITTER_PID
EXIT_CODE=$?

log_error "A component pipeline exited with code $EXIT_CODE"

for pid_name in "s2e-listener:$PIPE_S2E_LISTENER_PID" "s2e-submitter:$PIPE_S2E_SUBMITTER_PID" "e2s-listener:$PIPE_E2S_LISTENER_PID" "e2s-submitter:$PIPE_E2S_SUBMITTER_PID"; do
    name="${pid_name%%:*}"
    pid="${pid_name##*:}"
    if ! kill -0 "$pid" 2>/dev/null; then
        log_error "  $name (PID=$pid) has exited"
    else
        log "  $name (PID=$pid) still running, sending SIGTERM"
        kill "$pid" 2>/dev/null || true
    fi
done

# 等待剩余进程优雅退出
sleep 2
log "Container exiting (code=$EXIT_CODE)"
exit "$EXIT_CODE"
