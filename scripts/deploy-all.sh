#!/bin/bash

# 完整部署脚本：部署所有组件到本地测试网

set -e

GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

log_section() {
    echo -e "\n${BLUE}╔══════════════════════════════════════════╗${NC}"
    printf "${BLUE}║${NC}  %-40s${BLUE}║${NC}\n" "$1"
    echo -e "${BLUE}╚══════════════════════════════════════════╝${NC}\n"
}

log_info() {
    echo -e "${GREEN}✓${NC} $1"
}

cd /workspace

log_section "多签跨链桥 - 完整部署"

# Step 1: 启动测试网
log_section "Step 1: 启动测试网"
./scripts/start-testnet.sh > /tmp/testnet-start.log 2>&1 &
sleep 8

if curl -s http://localhost:8545 > /dev/null; then
    log_info "Anvil (EVM) 运行中"
else
    echo "❌ Anvil 未启动"
    exit 1
fi

if curl -s http://localhost:8899 > /dev/null; then
    log_info "Solana Test Validator 运行中"
else
    log_info "Solana 未启动（跳过）"
fi

# Step 2: 部署 EVM 合约
log_section "Step 2: 部署 EVM 合约"
cd /workspace/contracts/evm

CONTRACT=$(forge script script/Deploy.s.sol \
    --rpc-url http://localhost:8545 \
    --broadcast 2>&1 | grep "CoreContract deployed" | awk '{print $4}')

log_info "CoreContract: $CONTRACT"

# 更新Guardian配置中的合约地址
sed -i "s/core_contract = \".*\"/core_contract = \"$CONTRACT\"/" \
    /workspace/guardian/configs/local.toml

log_info "Guardian配置已更新"

# Step 3: 部署 Solana 程序
log_section "Step 3: 部署 Solana 程序"
cd /workspace/programs/solana-core

if solana cluster-version > /dev/null 2>&1; then
    PROGRAM_ID=$(solana-keygen pubkey target/deploy/solana_core-keypair.json)
    anchor deploy > /dev/null 2>&1
    log_info "Solana Program: $PROGRAM_ID"
    
    # 更新Guardian配置
    sed -i "s/core_program = \".*\"/core_program = \"$PROGRAM_ID\"/" \
        /workspace/guardian/configs/local.toml
    log_info "Solana配置已更新"
else
    log_info "Solana未运行，跳过程序部署"
fi

log_section "部署完成"

cat << EOF
${GREEN}✅ 所有组件部署完成！${NC}

部署信息:
  EVM Contract:    $CONTRACT
  Chain ID:        1337 (Anvil)
  
配置文件已更新:
  guardian/configs/local.toml

下一步:
  1. 启动Guardian网络: ./scripts/start-guardians.sh
  2. 测试完整流程: ./tests/e2e-test.sh

EOF

