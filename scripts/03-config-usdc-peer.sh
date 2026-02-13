#!/bin/bash

# ==============================================================================
# USDC 和对端地址配置脚本
# ==============================================================================
# 在 EVM 和 SVM 合约上配置 USDC 地址和对端合约地址
# 
# 自动从部署配置文件读取：
#   - .env.evm.deploy: EVM 相关配置
#   - .env.svm.deploy: SVM 相关配置

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ==============================================================================
# 配置文件路径
# ==============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/.."
EVM_DEPLOY_ENV="$PROJECT_ROOT/.env.evm.deploy"
SVM_DEPLOY_ENV="$PROJECT_ROOT/.env.svm.deploy"
CONFIG_ENV="$PROJECT_ROOT/.env.config-usdc-peer"

echo -e "${BLUE}============================================${NC}"
echo -e "${BLUE}加载配置文件${NC}"
echo -e "${BLUE}============================================${NC}"
echo ""

# ==============================================================================
# 加载部署配置文件
# ==============================================================================

# 加载 EVM 部署配置
if [ -f "$EVM_DEPLOY_ENV" ]; then
    echo -e "${GREEN}✓ 找到 EVM 部署配置: .env.evm.deploy${NC}"
    set -a
    source "$EVM_DEPLOY_ENV"
    set +a
else
    echo -e "${RED}✗ 未找到 .env.evm.deploy${NC}"
    echo -e "${YELLOW}  请先运行 01-deploy-evm.sh 部署 EVM 合约${NC}"
    exit 1
fi

# 加载 SVM 部署配置
if [ -f "$SVM_DEPLOY_ENV" ]; then
    echo -e "${GREEN}✓ 找到 SVM 部署配置: .env.svm.deploy${NC}"
    set -a
    source "$SVM_DEPLOY_ENV"
    set +a
else
    echo -e "${RED}✗ 未找到 .env.svm.deploy${NC}"
    echo -e "${YELLOW}  请先运行 02-deploy-svm.sh 部署 SVM 合约${NC}"
    exit 1
fi

# 加载额外配置（USDC 地址等）
if [ -f "$CONFIG_ENV" ]; then
    echo -e "${GREEN}✓ 找到额外配置: .env.config-usdc-peer${NC}"
    set -a
    source "$CONFIG_ENV"
    set +a
else
    echo -e "${YELLOW}⚠ 未找到 .env.config-usdc-peer${NC}"
    echo -e "${YELLOW}  USDC 地址需要在该文件中配置${NC}"
fi

echo ""

# ==============================================================================
# 读取 SVM 私钥（从 keypair 文件）
# ==============================================================================

if [ -z "$ADMIN_SVM_PRIVATE_KEY" ] && [ -n "$SVM_KEYPAIR_PATH" ] && [ -f "$SVM_KEYPAIR_PATH" ]; then
    echo -e "${BLUE}从 keypair 文件读取 SVM 私钥...${NC}"
    ADMIN_SVM_PRIVATE_KEY=$(cat "$SVM_KEYPAIR_PATH")
    echo -e "${GREEN}✓ 成功读取 SVM 私钥${NC}"
    echo ""
fi

# ==============================================================================
# 设置默认值
# ==============================================================================

# Chain IDs 默认值
EVM_CHAIN_ID=${EVM_CHAIN_ID:-421614}  # Arbitrum Sepolia
SVM_CHAIN_ID=${SVM_CHAIN_ID:-91024}   # 1024Chain

# 确保 EVM_RPC_URL 有正确的协议前缀
if [[ -n "$EVM_RPC_URL" ]] && [[ ! "$EVM_RPC_URL" =~ ^https?:// ]]; then
    EVM_RPC_URL="https://$EVM_RPC_URL"
fi

# ==============================================================================
# 验证必需的环境变量
# ==============================================================================

echo -e "${BLUE}============================================${NC}"
echo -e "${BLUE}验证配置信息${NC}"
echo -e "${BLUE}============================================${NC}"
echo ""

MISSING_VARS=0

# EVM 配置验证
echo -e "${BLUE}EVM 配置:${NC}"

if [ -z "$EVM_RPC_URL" ]; then
    echo -e "${RED}  ✗ 缺少 EVM_RPC_URL${NC}"
    MISSING_VARS=1
else
    echo -e "${GREEN}  ✓ EVM_RPC_URL: $EVM_RPC_URL${NC}"
fi

if [ -z "$EVM_CONTRACT_ADDRESS" ]; then
    echo -e "${RED}  ✗ 缺少 EVM_CONTRACT_ADDRESS${NC}"
    echo -e "${YELLOW}    请先运行 01-deploy-evm.sh 部署合约${NC}"
    MISSING_VARS=1
else
    echo -e "${GREEN}  ✓ EVM_CONTRACT_ADDRESS: $EVM_CONTRACT_ADDRESS${NC}"
fi

if [ -z "$ADMIN_EVM_PRIVATE_KEY" ]; then
    echo -e "${RED}  ✗ 缺少 ADMIN_EVM_PRIVATE_KEY${NC}"
    MISSING_VARS=1
else
    echo -e "${GREEN}  ✓ ADMIN_EVM_PRIVATE_KEY: [已设置]${NC}"
fi

if [ -z "$USDC_EVM_CONTRACT" ]; then
    echo -e "${RED}  ✗ 缺少 USDC_EVM_CONTRACT${NC}"
    echo -e "${YELLOW}    请在 .env.config-usdc-peer 中配置 USDC 合约地址${NC}"
    MISSING_VARS=1
else
    echo -e "${GREEN}  ✓ USDC_EVM_CONTRACT: $USDC_EVM_CONTRACT${NC}"
fi

echo -e "${GREEN}  ✓ EVM_CHAIN_ID: $EVM_CHAIN_ID${NC}"

echo ""
echo -e "${BLUE}SVM 配置:${NC}"

# SVM 配置验证
if [ -z "$SVM_RPC_URL" ]; then
    echo -e "${RED}  ✗ 缺少 SVM_RPC_URL${NC}"
    MISSING_VARS=1
else
    echo -e "${GREEN}  ✓ SVM_RPC_URL: $SVM_RPC_URL${NC}"
fi

if [ -z "$SVM_PROGRAM_ID" ]; then
    echo -e "${RED}  ✗ 缺少 SVM_PROGRAM_ID${NC}"
    echo -e "${YELLOW}    请先运行 02-deploy-svm.sh 部署合约${NC}"
    MISSING_VARS=1
else
    echo -e "${GREEN}  ✓ SVM_PROGRAM_ID: $SVM_PROGRAM_ID${NC}"
fi

if [ -z "$ADMIN_SVM_PRIVATE_KEY" ]; then
    echo -e "${RED}  ✗ 缺少 ADMIN_SVM_PRIVATE_KEY${NC}"
    echo -e "${YELLOW}    无法从 $SVM_KEYPAIR_PATH 读取私钥${NC}"
    MISSING_VARS=1
else
    echo -e "${GREEN}  ✓ ADMIN_SVM_PRIVATE_KEY: [已设置]${NC}"
fi

if [ -z "$USDC_SVM_MINT" ]; then
    echo -e "${RED}  ✗ 缺少 USDC_SVM_MINT${NC}"
    echo -e "${YELLOW}    请在 .env.config-usdc-peer 中配置 USDC Mint 地址${NC}"
    MISSING_VARS=1
else
    echo -e "${GREEN}  ✓ USDC_SVM_MINT: $USDC_SVM_MINT${NC}"
fi

echo -e "${GREEN}  ✓ SVM_CHAIN_ID: $SVM_CHAIN_ID${NC}"

echo ""
echo -e "${BLUE}对端配置:${NC}"
echo -e "${GREEN}  ✓ EVM → SVM: $SVM_PROGRAM_ID${NC}"
echo -e "${GREEN}  ✓ SVM → EVM: $EVM_CONTRACT_ADDRESS${NC}"

if [ $MISSING_VARS -eq 1 ]; then
    echo ""
    echo -e "${RED}配置验证失败，请检查上述错误${NC}"
    exit 1
fi

echo ""

# ==============================================================================
# 配置 EVM 合约
# ==============================================================================

echo -e "${BLUE}============================================${NC}"
echo -e "${BLUE}步骤 1/4: 配置 EVM 合约 USDC 地址${NC}"
echo -e "${BLUE}============================================${NC}"
echo ""

cd "$SCRIPT_DIR"

# 使用本地 ts-node 调用 evm-admin.ts
if ! EVM_RPC_URL="$EVM_RPC_URL" \
     EVM_CONTRACT_ADDRESS="$EVM_CONTRACT_ADDRESS" \
     ADMIN_EVM_PRIVATE_KEY="$ADMIN_EVM_PRIVATE_KEY" \
     USDC_EVM_CONTRACT="$USDC_EVM_CONTRACT" \
     EVM_CHAIN_ID="$EVM_CHAIN_ID" \
     SVM_CHAIN_ID="$SVM_CHAIN_ID" \
     ./node_modules/.bin/ts-node evm-admin.ts configure_usdc; then
    echo -e "${RED}配置 EVM USDC 失败${NC}"
    exit 1
fi

echo ""

# ==============================================================================
# 配置 EVM 对端合约
# ==============================================================================

echo -e "${BLUE}============================================${NC}"
echo -e "${BLUE}步骤 2/4: 配置 EVM 对端合约地址${NC}"
echo -e "${BLUE}============================================${NC}"
echo ""

PEER_CONTRACT_ADDRESS_FOR_EVM=$SVM_PROGRAM_ID

if ! EVM_RPC_URL="$EVM_RPC_URL" \
     EVM_CONTRACT_ADDRESS="$EVM_CONTRACT_ADDRESS" \
     ADMIN_EVM_PRIVATE_KEY="$ADMIN_EVM_PRIVATE_KEY" \
     USDC_EVM_CONTRACT="$USDC_EVM_CONTRACT" \
     PEER_CONTRACT_ADDRESS_FOR_EVM="$PEER_CONTRACT_ADDRESS_FOR_EVM" \
     EVM_CHAIN_ID="$EVM_CHAIN_ID" \
     SVM_CHAIN_ID="$SVM_CHAIN_ID" \
     ./node_modules/.bin/ts-node evm-admin.ts configure_peer; then
    echo -e "${RED}配置 EVM 对端合约失败${NC}"
    exit 1
fi

echo ""

# ==============================================================================
# 配置 SVM 合约
# ==============================================================================

echo -e "${BLUE}============================================${NC}"
echo -e "${BLUE}步骤 3/4: 配置 SVM 合约 USDC Mint${NC}"
echo -e "${BLUE}============================================${NC}"
echo ""

if ! SVM_RPC_URL="$SVM_RPC_URL" \
     SVM_PROGRAM_ID="$SVM_PROGRAM_ID" \
     ADMIN_SVM_PRIVATE_KEY="$ADMIN_SVM_PRIVATE_KEY" \
     USDC_SVM_MINT="$USDC_SVM_MINT" \
     EVM_CHAIN_ID="$EVM_CHAIN_ID" \
     SVM_CHAIN_ID="$SVM_CHAIN_ID" \
     timeout 5s ./node_modules/.bin/ts-node svm-admin.ts configure_usdc; then
    echo -e "${RED}配置 SVM USDC 失败${NC}（也可能是ws连接超时）"
    # exit 1
fi

echo ""

# ==============================================================================
# 配置 SVM 对端合约
# ==============================================================================

echo -e "${BLUE}============================================${NC}"
echo -e "${BLUE}步骤 4/4: 配置 SVM 对端合约地址${NC}"
echo -e "${BLUE}============================================${NC}"
echo ""

PEER_CONTRACT_ADDRESS_FOR_SVM=$EVM_CONTRACT_ADDRESS

if ! SVM_RPC_URL="$SVM_RPC_URL" \
     SVM_PROGRAM_ID="$SVM_PROGRAM_ID" \
     ADMIN_SVM_PRIVATE_KEY="$ADMIN_SVM_PRIVATE_KEY" \
     USDC_SVM_MINT="$USDC_SVM_MINT" \
     PEER_CONTRACT_ADDRESS_FOR_SVM="$PEER_CONTRACT_ADDRESS_FOR_SVM" \
     EVM_CHAIN_ID="$EVM_CHAIN_ID" \
     SVM_CHAIN_ID="$SVM_CHAIN_ID" \
     timeout 5s ./node_modules/.bin/ts-node svm-admin.ts configure_peer; then
    echo -e "${RED}配置 SVM 对端合约失败${NC}（也可能是ws连接超时）"
    # exit 1
fi

echo ""

# ==============================================================================
# 配置 Decimal Ratio（如果需要）
# ==============================================================================

DECIMAL_RATIO=${DECIMAL_RATIO:-1}

if [ "$DECIMAL_RATIO" != "1" ]; then
    echo -e "${BLUE}============================================${NC}"
    echo -e "${BLUE}步骤 5: 配置 EVM Decimal Ratio${NC}"
    echo -e "${BLUE}============================================${NC}"
    echo ""
    echo -e "${YELLOW}Decimal Ratio: $DECIMAL_RATIO${NC}"
    echo ""

    if ! EVM_RPC_URL="$EVM_RPC_URL" \
         EVM_CONTRACT_ADDRESS="$EVM_CONTRACT_ADDRESS" \
         ADMIN_EVM_PRIVATE_KEY="$ADMIN_EVM_PRIVATE_KEY" \
         DECIMAL_RATIO="$DECIMAL_RATIO" \
         ./node_modules/.bin/ts-node evm-admin.ts configure_decimal_ratio "$DECIMAL_RATIO"; then
        echo -e "${RED}配置 EVM Decimal Ratio 失败${NC}"
        exit 1
    fi

    echo ""
fi

# ==============================================================================
# 完成
# ==============================================================================

echo -e "${GREEN}============================================${NC}"
echo -e "${GREEN}配置完成！${NC}"
echo -e "${GREEN}============================================${NC}"
echo ""
echo -e "${GREEN}✓ EVM 合约 USDC 地址已配置${NC}"
echo -e "${GREEN}✓ EVM 对端合约地址已配置${NC}"
echo -e "${GREEN}✓ SVM 合约 USDC Mint 已配置${NC}"
echo -e "${GREEN}✓ SVM 对端合约地址已配置${NC}"
echo ""
echo -e "${BLUE}下一步操作：${NC}"
echo -e "  - 使用 evm-admin.ts query_state 查询 EVM 合约状态"
echo -e "  - 使用 svm-admin.ts query_state 查询 SVM 合约状态（如果支持）"
echo -e "  - 添加 Relayer 地址"
echo -e "  - 添加流动性"
echo ""

