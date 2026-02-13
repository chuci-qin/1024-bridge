#!/bin/bash
set -e

# ============================================
# Relayer 容器入口脚本
# 功能：
# 1. 从带前缀的环境变量生成 3 个组件的 .env 文件
# 2. 启动 s2e、e2s-listener、e2s-submitter 三个组件
# 3. 监控子进程，任一退出则容器退出（触发 Easypanel 自动重启）
# ============================================

APP_DIR="/app"

# 从带前缀的环境变量提取并写入 .env 文件
# 用法: extract_env <PREFIX> <OUTPUT_FILE>
# 示例: extract_env "S2E_" "/app/s2e/.env"
#   S2E_SERVICE__NAME=s2e  ->  SERVICE__NAME=s2e
extract_env() {
    local prefix="$1"
    local output="$2"
    local prefix_len=${#prefix}

    > "$output"  # 清空文件

    while IFS='=' read -r key value; do
        # 检查是否以指定前缀开头
        if [[ "$key" == ${prefix}* ]]; then
            # 去除前缀，写入 .env 文件
            local stripped_key="${key:$prefix_len}"
            echo "${stripped_key}=${value}" >> "$output"
        fi
    done < <(env)

    echo "[entrypoint] Generated $output ($(wc -l < "$output") vars)"
}

# 生成 3 个组件的 .env 文件
echo "[entrypoint] Generating .env files from prefixed environment variables..."
extract_env "S2E_" "$APP_DIR/s2e/.env"
extract_env "E2S_LISTENER_" "$APP_DIR/e2s-listener/.env"
extract_env "E2S_SUBMITTER_" "$APP_DIR/e2s-submitter/.env"

echo "[entrypoint] Starting relayer components..."

# 启动 s2e
cd $APP_DIR/s2e
cargo run --release > "$APP_DIR/logs/s2e.log" 2>&1 &
S2E_PID=$!
echo "[entrypoint] s2e started (PID: $S2E_PID)"

# 启动 e2s-listener
cd $APP_DIR/e2s-listener
cargo run --release > "$APP_DIR/logs/e2s-listener.log" 2>&1 &
E2S_LISTENER_PID=$!
echo "[entrypoint] e2s-listener started (PID: $E2S_LISTENER_PID)"

# 启动 e2s-submitter
cd $APP_DIR/e2s-submitter
cargo run --release > "$APP_DIR/logs/e2s-submitter.log" 2>&1 &
E2S_SUBMITTER_PID=$!
echo "[entrypoint] e2s-submitter started (PID: $E2S_SUBMITTER_PID)"

echo "[entrypoint] All components started. Monitoring processes..."

# 监控子进程：任一退出则容器退出
# Easypanel 的 restart policy 会自动重启容器
wait -n $S2E_PID $E2S_LISTENER_PID $E2S_SUBMITTER_PID
EXIT_CODE=$?

echo "[entrypoint] A component exited with code $EXIT_CODE. Stopping all components..."

# 清理：停止其他进程
kill $S2E_PID $E2S_LISTENER_PID $E2S_SUBMITTER_PID 2>/dev/null || true
wait 2>/dev/null || true

echo "[entrypoint] Container exiting with code $EXIT_CODE"
exit $EXIT_CODE
