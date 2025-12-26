#!/bin/bash

# ==============================================================================
# EVM 合约部署脚本 - Arbitrum Sepolia
# ==============================================================================

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 显示欢迎信息
clear 2>/dev/null || true
echo -e "${BLUE}╔════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║                                                        ║${NC}"
echo -e "${BLUE}║         EVM 合约部署脚本 (Arbitrum Sepolia)           ║${NC}"
echo -e "${BLUE}║                    Bridge1024 v2.0                     ║${NC}"
echo -e "${BLUE}║                                                        ║${NC}"
echo -e "${BLUE}╚════════════════════════════════════════════════════════╝${NC}"
echo ""

# ==============================================================================
# 配置
# ==============================================================================

# 获取项目路径
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/.."
CONTRACT_DIR="$PROJECT_ROOT/evm/bridge1024"
ENV_FILE="$PROJECT_ROOT/.env.evm.deploy"

# 从 .env.evm.deploy 文件加载环境变量
if [ -f "$ENV_FILE" ]; then
    set -a  # 自动导出所有变量
    source "$ENV_FILE"
    set +a  # 关闭自动导出
else
    echo -e "${RED}未找到 .env.evm.deploy 文件${NC}"
    exit 1
fi

# 读取环境变量
RPC_URL="${EVM_RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}"

# 确保 RPC URL 有正确的协议前缀
if [[ ! "$RPC_URL" =~ ^https?:// ]]; then
    RPC_URL="https://$RPC_URL"
fi

PRIVATE_KEY="${ADMIN_EVM_PRIVATE_KEY}"
ADMIN_ADDRESS="${EVM_ADMIN_ADDRESS}"

# ==============================================================================
# 检查环境变量
# ==============================================================================

if [ -z "$PRIVATE_KEY" ]; then
    echo -e "${RED}未设置 ADMIN_EVM_PRIVATE_KEY${NC}"
    exit 1
fi

if [ -z "$ADMIN_ADDRESS" ]; then
    ADMIN_ADDRESS=$(cast wallet address "$PRIVATE_KEY" 2>/dev/null || echo "")
fi

# 注意：从 v2.0 开始，合约本身作为金库，不需要单独的 vault 地址

# ==============================================================================
# 部署合约
# ==============================================================================

echo ""
echo -e "${YELLOW}==== 开始部署 EVM 合约 ====${NC}"
echo ""

cd "$CONTRACT_DIR" || exit 1

# 编译合约
echo -e "${YELLOW}[1/4] 编译合约...${NC}"
BUILD_OUTPUT=$(mktemp)
if ! forge build > "$BUILD_OUTPUT" 2>&1; then
    echo -e "${RED}✗ 编译失败${NC}"
    cat "$BUILD_OUTPUT"
    rm -f "$BUILD_OUTPUT"
    exit 1
fi
rm -f "$BUILD_OUTPUT"
echo -e "${GREEN}✓ 编译成功${NC}"

# 部署Bridge1024
echo -e "${YELLOW}[2/4] 部署合约...${NC}"
TEMP_OUTPUT=$(mktemp)

forge create \
    --rpc-url "$RPC_URL" \
    --private-key "$PRIVATE_KEY" \
    --broadcast \
    src/Bridge1024.sol:Bridge1024 \
    > "$TEMP_OUTPUT" 2>&1

DEPLOY_EXIT_CODE=$?

if [ $DEPLOY_EXIT_CODE -ne 0 ]; then
    echo -e "${RED}✗ 部署失败${NC}"
    cat "$TEMP_OUTPUT"
    rm -f "$TEMP_OUTPUT"
    exit 1
fi

# 从输出中提取交易哈希（兼容 macOS）
DEPLOY_TX=$(grep "Transaction hash:" "$TEMP_OUTPUT" | sed -E 's/.*Transaction hash:[[:space:]]*(0x[a-fA-F0-9]{64}).*/\1/' | head -1)

# 从输出中提取合约地址（兼容 macOS）
CONTRACT_ADDRESS=$(grep "Deployed to:" "$TEMP_OUTPUT" | sed -E 's/.*Deployed to:[[:space:]]*(0x[a-fA-F0-9]{40}).*/\1/' | head -1)

if [ -z "$CONTRACT_ADDRESS" ]; then
    echo -e "${RED}✗ 无法提取合约地址${NC}"
    cat "$TEMP_OUTPUT"
    rm -f "$TEMP_OUTPUT"
    exit 1
fi

rm -f "$TEMP_OUTPUT"

# 输出部署信息
echo -e "${GREEN}✓ 部署成功${NC}"
echo ""
echo -e "  ${YELLOW}部署交易:${NC} ${DEPLOY_TX}"
echo -e "  ${YELLOW}合约地址:${NC} ${CONTRACT_ADDRESS}"
echo -e "  ${YELLOW}区块浏览器:${NC} https://sepolia.arbiscan.io/address/${CONTRACT_ADDRESS}"
echo ""

# ==============================================================================
# 初始化合约
# ==============================================================================

echo -e "${YELLOW}[3/4] 初始化合约...${NC}"

# 注意：合约内部使用 address(this) 作为金库，不需要单独的 vault 地址
INIT_OUTPUT=$(cast send "$CONTRACT_ADDRESS" \
    "initialize(address)" \
    "$ADMIN_ADDRESS" \
    --rpc-url "$RPC_URL" \
    --private-key "$PRIVATE_KEY" \
    2>&1)

if [ $? -ne 0 ]; then
    echo -e "${RED}✗ 初始化失败${NC}"
    echo "$INIT_OUTPUT" | grep -i "error"
    exit 1
fi

# 提取初始化交易哈希（兼容 macOS）
INIT_TX=$(echo "$INIT_OUTPUT" | grep "transactionHash" | sed -E 's/.*transactionHash[[:space:]]+(0x[a-fA-F0-9]{64}).*/\1/' | head -1)

echo -e "${GREEN}✓ 初始化成功${NC}"
echo -e "  ${YELLOW}初始化交易:${NC} ${INIT_TX}"
echo ""

# 保存旧合约地址用于后续替换
OLD_EVM_CONTRACT_ADDRESS=$(grep "EVM_CONTRACT_ADDRESS=" "$ENV_FILE" 2>/dev/null | sed -E 's/.*EVM_CONTRACT_ADDRESS=(0x[a-fA-F0-9]{40}).*/\1/' | head -1)

# 保存到环境变量文件
echo -e "${YELLOW}[4/4] 保存配置...${NC}"
if [ -f "$ENV_FILE" ]; then
    if grep -q "EVM_CONTRACT_ADDRESS=" "$ENV_FILE"; then
        sed -i "s|EVM_CONTRACT_ADDRESS=.*|EVM_CONTRACT_ADDRESS=${CONTRACT_ADDRESS}|g" "$ENV_FILE"
    else
        echo "EVM_CONTRACT_ADDRESS=${CONTRACT_ADDRESS}" >> "$ENV_FILE"
    fi
    echo -e "${GREEN}✓ 配置已保存到 .env.evm.deploy${NC}"
fi

# ==============================================================================
# 全项目同步配置：替换旧合约地址为新地址
# ==============================================================================

if [ -n "$OLD_EVM_CONTRACT_ADDRESS" ] && [ "$OLD_EVM_CONTRACT_ADDRESS" != "$CONTRACT_ADDRESS" ]; then
    echo ""
    echo -e "${YELLOW}正在全项目替换合约地址...${NC}"
    echo -e "  旧地址: ${OLD_EVM_CONTRACT_ADDRESS}"
    echo -e "  新地址: ${CONTRACT_ADDRESS}"
    
    # 设置 LC_ALL=C 避免编码问题
    export LC_ALL=C
    
    # 查找并替换所有文件中的旧地址
    REPLACED_COUNT=$(find "$PROJECT_ROOT" \
        -type f \
        ! -path "*/.git/*" \
        ! -path "*/node_modules/*" \
        ! -path "*/cache/*" \
        ! -path "*/.venv/*" \
        ! -path "*/out/*" \
        ! -path "*/target/*" \
        ! -path "*/.next/*" \
        ! -name "*.log" \
        -exec grep -l "$OLD_EVM_CONTRACT_ADDRESS" {} \; 2>/dev/null | \
        while read file; do
            sed -i "s|$OLD_EVM_CONTRACT_ADDRESS|$CONTRACT_ADDRESS|g" "$file"
            echo "$file"
        done | wc -l)
    
    echo -e "${GREEN}✓ 已更新 ${REPLACED_COUNT} 个文件${NC}"
else
    echo ""
    echo -e "${YELLOW}跳过地址替换（首次部署或地址未变）${NC}"
fi

# ==============================================================================
# 完成
# ==============================================================================

echo ""
echo -e "${GREEN}==== EVM 合约部署完成 ====${NC}"
echo ""
echo -e "${GREEN}✓ 合约地址:${NC} ${CONTRACT_ADDRESS}"
echo -e "${GREEN}✓ 管理员地址:${NC} ${ADMIN_ADDRESS}"
echo -e "${GREEN}✓ 区块浏览器:${NC} https://sepolia.arbiscan.io/address/${CONTRACT_ADDRESS}"
echo ""
echo -e "${YELLOW}下一步:${NC}"
echo -e "  1. 运行 ${YELLOW}./02-deploy-svm.sh${NC} 部署 SVM 合约"
echo -e "  2. 运行 ${YELLOW}./03-config-usdc-peer.sh${NC} 配置跨链对等"
echo -e "  3. 运行 ${YELLOW}./04-register-static-relayers.sh${NC} 注册中继器"
echo ""
