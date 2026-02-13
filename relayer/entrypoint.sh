#!/bin/bash

# ============================================
# Relayer 容器入口脚本（调试增强版）
# 功能：
# 1. 从带前缀的环境变量生成 3 个组件的 .env 文件
# 2. 启动 s2e、e2s-listener、e2s-submitter 三个组件
# 3. 监控子进程，任一退出则容器退出（触发 Easypanel 自动重启）
# ============================================

# 不要用 set -e，否则 wait -n 非零退出会导致脚本直接退出，跳过诊断输出
# set -e

APP_DIR="/app"
LOG_DIR="$APP_DIR/logs"
mkdir -p "$LOG_DIR"

# 带时间戳的日志函数
log() {
    echo "[entrypoint][$(date '+%Y-%m-%d %H:%M:%S')] $*"
}

# 从带前缀的环境变量提取并写入 .env 文件
# 用法: extract_env <PREFIX> <OUTPUT_FILE>
# 示例: extract_env "S2E_" "/app/s2e/.env"
#   S2E_SERVICE__NAME=s2e  ->  SERVICE__NAME=s2e
extract_env() {
    local prefix="$1"
    local output="$2"
    local prefix_len=${#prefix}

    > "$output"  # 清空文件

    # 使用 printenv 而不是 env，避免 bash 函数导出等干扰
    while IFS='=' read -r key value; do
        # 检查是否以指定前缀开头
        if [[ "$key" == ${prefix}* ]]; then
            # 去除前缀，写入 .env 文件
            local stripped_key="${key:$prefix_len}"
            echo "${stripped_key}=${value}" >> "$output"
        fi
    done < <(printenv)

    log "Generated $output ($(wc -l < "$output") vars)"
}

# ============================================
# 打印系统信息
# ============================================
log "===== System Info ====="
log "Hostname: $(hostname)"
log "Kernel: $(uname -r)"
log "Memory: $(free -h 2>/dev/null | head -2 || echo 'N/A')"
log "Disk: $(df -h /app 2>/dev/null | tail -1 || echo 'N/A')"
log "========================"

# ============================================
# 生成 .env 文件
# ============================================
log "Generating .env files from prefixed environment variables..."
extract_env "S2E_" "$APP_DIR/s2e/.env"
extract_env "E2S_LISTENER_" "$APP_DIR/e2s-listener/.env"
extract_env "E2S_SUBMITTER_" "$APP_DIR/e2s-submitter/.env"

# 打印生成的 .env 内容（脱敏：隐藏私钥等敏感值）
log "===== Generated .env contents (sensitive values masked) ====="
for envfile in "$APP_DIR/s2e/.env" "$APP_DIR/e2s-listener/.env" "$APP_DIR/e2s-submitter/.env"; do
    log "--- $envfile ---"
    while IFS= read -r line; do
        key="${line%%=*}"
        value="${line#*=}"
        # 对可能包含敏感信息的 key 进行脱敏
        if echo "$key" | grep -iqE 'key|secret|password|private|mnemonic|token'; then
            log "  $key=****MASKED****"
        else
            log "  $line"
        fi
    done < "$envfile"
done
log "============================================================="

# ============================================
# 检查二进制文件是否存在
# ============================================
S2E_BIN="$APP_DIR/s2e/target/release/s2e-relayer"
E2S_LISTENER_BIN="$APP_DIR/e2s-listener/target/release/e2s-listener"
E2S_SUBMITTER_BIN="$APP_DIR/e2s-submitter/target/release/e2s-submitter"

log "===== Checking binaries ====="
for bin in "$S2E_BIN" "$E2S_LISTENER_BIN" "$E2S_SUBMITTER_BIN"; do
    if [ -f "$bin" ]; then
        log "  OK: $bin ($(ls -lh "$bin" | awk '{print $5}'))"
    else
        log "  MISSING: $bin"
        log "  ERROR: Binary not found! Listing directory:"
        ls -la "$(dirname "$bin")" 2>&1 | while read -r line; do log "    $line"; done
    fi
done
log "=============================="

# ============================================
# 启动组件：每个组件在独立子 shell 中运行
# 关键：从 .env 文件 export 变量到子 shell 环境，不再依赖 dotenvy 从文件加载
# 这样 config-rs 的 Environment::default().separator("__") 能直接从进程环境读取配置
# ============================================
log "Starting relayer components..."

# 启动 s2e（子 shell 中 export .env 变量 + exec 二进制）
log "Starting s2e (exporting env vars from .env)..."
( while IFS='=' read -r k v; do export "$k=$v"; done < "$APP_DIR/s2e/.env"; cd "$APP_DIR/s2e"; exec "$S2E_BIN" ) 2>&1 | tee "$LOG_DIR/s2e.log" | sed 's/^/[s2e] /' &
S2E_PID=$!
log "s2e started (PID: $S2E_PID)"

# 启动 e2s-listener
log "Starting e2s-listener (exporting env vars from .env)..."
( while IFS='=' read -r k v; do export "$k=$v"; done < "$APP_DIR/e2s-listener/.env"; cd "$APP_DIR/e2s-listener"; exec "$E2S_LISTENER_BIN" ) 2>&1 | tee "$LOG_DIR/e2s-listener.log" | sed 's/^/[e2s-listener] /' &
E2S_LISTENER_PID=$!
log "e2s-listener started (PID: $E2S_LISTENER_PID)"

# 启动 e2s-submitter
log "Starting e2s-submitter (exporting env vars from .env)..."
( while IFS='=' read -r k v; do export "$k=$v"; done < "$APP_DIR/e2s-submitter/.env"; cd "$APP_DIR/e2s-submitter"; exec "$E2S_SUBMITTER_BIN" ) 2>&1 | tee "$LOG_DIR/e2s-submitter.log" | sed 's/^/[e2s-submitter] /' &
E2S_SUBMITTER_PID=$!
log "e2s-submitter started (PID: $E2S_SUBMITTER_PID)"

log "All components started. Monitoring processes..."
log "  s2e PID=$S2E_PID, e2s-listener PID=$E2S_LISTENER_PID, e2s-submitter PID=$E2S_SUBMITTER_PID"

# ============================================
# 短暂等待后检查进程是否仍然存活（捕获立即崩溃的情况）
# ============================================
sleep 3
log "===== Process health check after 3s ====="
for name_pid in "s2e:$S2E_PID" "e2s-listener:$E2S_LISTENER_PID" "e2s-submitter:$E2S_SUBMITTER_PID"; do
    name="${name_pid%%:*}"
    pid="${name_pid##*:}"
    if kill -0 "$pid" 2>/dev/null; then
        log "  $name (PID $pid): RUNNING"
    else
        wait "$pid" 2>/dev/null
        exit_code=$?
        log "  $name (PID $pid): ALREADY EXITED (code: $exit_code)"
        log "  === Last 50 lines of $name log ==="
        tail -50 "$LOG_DIR/$name.log" 2>/dev/null | while IFS= read -r line; do log "    [$name] $line"; done
        log "  === End of $name log ==="
    fi
done
log "==========================================="

# ============================================
# 监控子进程：任一退出则记录详细信息并退出容器
# ============================================
log "Entering main monitoring loop (wait -n)..."

wait -n "$S2E_PID" "$E2S_LISTENER_PID" "$E2S_SUBMITTER_PID"
WAIT_EXIT_CODE=$?

log "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
log "!! A component exited! wait -n returned code: $WAIT_EXIT_CODE"
log "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"

# 检测是哪个进程退出了
log "===== Identifying which process exited ====="
EXITED_NAME="unknown"
for name_pid in "s2e:$S2E_PID" "e2s-listener:$E2S_LISTENER_PID" "e2s-submitter:$E2S_SUBMITTER_PID"; do
    name="${name_pid%%:*}"
    pid="${name_pid##*:}"
    if kill -0 "$pid" 2>/dev/null; then
        log "  $name (PID $pid): still running"
    else
        wait "$pid" 2>/dev/null
        code=$?
        log "  $name (PID $pid): EXITED (code: $code)"
        EXITED_NAME="$name"
    fi
done

# 打印所有组件的最后日志
log "===== Log tails for all components ====="
for name in "s2e" "e2s-listener" "e2s-submitter"; do
    log "--- Last 100 lines of $name ---"
    tail -100 "$LOG_DIR/$name.log" 2>/dev/null | while IFS= read -r line; do log "  [$name] $line"; done
    log "--- End of $name ---"
done
log "==========================================="

# 清理：停止其他进程
log "Stopping remaining processes..."
kill "$S2E_PID" "$E2S_LISTENER_PID" "$E2S_SUBMITTER_PID" 2>/dev/null || true

# 给进程时间优雅退出
sleep 2

# 强制杀死仍在运行的进程
for name_pid in "s2e:$S2E_PID" "e2s-listener:$E2S_LISTENER_PID" "e2s-submitter:$E2S_SUBMITTER_PID"; do
    name="${name_pid%%:*}"
    pid="${name_pid##*:}"
    if kill -0 "$pid" 2>/dev/null; then
        log "Force killing $name (PID $pid)..."
        kill -9 "$pid" 2>/dev/null || true
    fi
done

wait 2>/dev/null || true

log "Container exiting. Crashed component: $EXITED_NAME, exit code: $WAIT_EXIT_CODE"
exit "$WAIT_EXIT_CODE"
