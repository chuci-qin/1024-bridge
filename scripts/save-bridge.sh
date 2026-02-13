#!/bin/bash
set -e
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
PROFILE="$1"
if [ -z "$PROFILE" ]; then
    echo -e "${RED}用法: ./save-bridge.sh <profile>${NC}"
    echo "  例如: ./save-bridge.sh arb-usdc / bnb-usdt / eth-usdc"
    exit 1
fi
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/.."
echo -e "${BLUE}保存桥配置: ${PROFILE}${NC}"
FILES=(".env.evm.deploy" ".env.svm.deploy" ".env.invoke" ".env.config-usdc-peer" "relayer/s2e/.env" "relayer/e2s-listener/.env" "relayer/e2s-submitter/.env")
SAVED=0
for FILE in "${FILES[@]}"; do
    SRC="$PROJECT_ROOT/$FILE"; DST="$PROJECT_ROOT/${FILE}.${PROFILE}"
    if [ -f "$SRC" ]; then
        cp "$SRC" "$DST"; echo -e "${GREEN}✓ ${FILE} -> ${FILE}.${PROFILE}${NC}"; SAVED=$((SAVED + 1))
    else
        echo -e "${YELLOW}⚠ 跳过: ${FILE}${NC}"
    fi
done
echo -e "${GREEN}完成！保存了 ${SAVED} 个文件${NC}"
