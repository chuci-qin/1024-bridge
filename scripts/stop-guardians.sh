#!/bin/bash

# 停止Guardian网络

set -e

GREEN='\033[0;32m'
NC='\033[0m'

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

cd /workspace

log_info "停止Guardian网络..."
docker-compose -f docker-compose.guardian.yml down

log_info "✅ Guardian网络已停止"

