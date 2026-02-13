#!/bin/bash
set -e
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
PROFILE="$1"
if [ -z "$PROFILE" ]; then
    echo -e "${RED}用法: ./switch-bridge.sh <profile>${NC}"
    echo "  例如: ./switch-bridge.sh arb-usdc / bnb-usdt / eth-usdc"
    echo "可用配置:"
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    PROJECT_ROOT="$SCRIPT_DIR/.."
    for f in "$PROJECT_ROOT"/.env.evm.deploy.*; do
        if [ -f "$f" ] && [[ ! "$f" == *.example ]]; then
            echo "  - $(basename "$f" | sed 's/^\.env\.evm\.deploy\.//')"
        fi
    done
    exit 1
fi
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/.."
echo -e "${BLUE}切换桥配置: ${PROFILE}${NC}"
FILES=(".env.evm.deploy" ".env.svm.deploy" ".env.invoke" ".env.config-usdc-peer" "relayer/s2e/.env" "relayer/e2s-listener/.env" "relayer/e2s-submitter/.env")
SWITCHED=0; MISSING=0
for FILE in "${FILES[@]}"; do
    SRC="$PROJECT_ROOT/${FILE}.${PROFILE}"; DST="$PROJECT_ROOT/$FILE"
    if [ -f "$SRC" ]; then
        cp "$SRC" "$DST"; echo -e "${GREEN}✓ ${FILE}.${PROFILE} -> ${FILE}${NC}"; SWITCHED=$((SWITCHED + 1))
    else
        echo -e "${YELLOW}⚠ 未找到: ${FILE}.${PROFILE}${NC}"; MISSING=$((MISSING + 1))
    fi
done
echo -e "${GREEN}完成！切换了 ${SWITCHED} 个文件${NC}"
