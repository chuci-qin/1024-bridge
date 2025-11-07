#!/bin/bash

# 停止本地测试网络脚本

set -e

# 颜色输出
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_info "Stopping local testnets..."

# 停止 Anvil
if [ -f /tmp/anvil.pid ]; then
    ANVIL_PID=$(cat /tmp/anvil.pid)
    if kill -0 $ANVIL_PID 2>/dev/null; then
        kill $ANVIL_PID
        log_info "Stopped Anvil (PID: $ANVIL_PID)"
    fi
    rm -f /tmp/anvil.pid
fi

# 停止 Solana
if [ -f /tmp/solana.pid ]; then
    SOLANA_PID=$(cat /tmp/solana.pid)
    if kill -0 $SOLANA_PID 2>/dev/null; then
        kill $SOLANA_PID
        log_info "Stopped Solana Test Validator (PID: $SOLANA_PID)"
    fi
    rm -f /tmp/solana.pid
fi

# 强制停止残留进程
pkill -f "anvil" || true
pkill -f "solana-test-validator" || true

log_info "✅ All testnets stopped"

