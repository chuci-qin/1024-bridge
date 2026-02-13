#!/bin/bash

# ============================================
# Relayer 账户充值脚本
# ============================================
# 功能：
# 1. 从 EVM 管理员账户转账 0.001 ETH 到 S2E Relayer 账户（用于支付 EVM gas 费用）
# 2. 为 E2S Relayer 账户申请 Solana airdrop（用于支付 SVM 交易费用）

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# ==============================================================================
# 初始化检查
# ==============================================================================

echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}  Relayer 账户充值脚本${NC}"
echo -e "${CYAN}========================================${NC}"
echo ""

# 加载环境变量
echo -e "${BLUE}步骤 1/4: 检查配置文件...${NC}"
if [ ! -f "$PROJECT_ROOT/.env.invoke" ]; then
    echo -e "${RED}❌ 错误: 未找到 .env.invoke 文件${NC}"
    echo -e "${YELLOW}提示: 请先运行部署脚本生成配置文件${NC}"
    exit 1
fi
source "$PROJECT_ROOT/.env.invoke"

# 检查 EVM 部署配置
if [ ! -f "$PROJECT_ROOT/.env.evm.deploy" ]; then
    echo -e "${RED}❌ 错误: 未找到 .env.evm.deploy 文件${NC}"
    exit 1
fi
source "$PROJECT_ROOT/.env.evm.deploy"

# 检查 SVM 部署配置
if [ ! -f "$PROJECT_ROOT/.env.svm.deploy" ]; then
    echo -e "${RED}❌ 错误: 未找到 .env.svm.deploy 文件${NC}"
    exit 1
fi
source "$PROJECT_ROOT/.env.svm.deploy"

echo -e "${GREEN}✓ 配置文件加载成功${NC}"
echo ""

# ==============================================================================
# 读取 Relayer 账户信息
# ==============================================================================

echo -e "${BLUE}步骤 2/4: 读取 Relayer 账户信息...${NC}"

# S2E Relayer (EVM)
S2E_ENV_PATH="$PROJECT_ROOT/relayer/s2e/.env"
if [ ! -f "$S2E_ENV_PATH" ]; then
    echo -e "${RED}❌ 错误: 未找到 S2E Relayer 配置文件: $S2E_ENV_PATH${NC}"
    echo -e "${YELLOW}提示: 请先运行 04-register-relayer.sh 注册 Relayer${NC}"
    exit 1
fi

S2E_PRIVATE_KEY=$(grep "^RELAYER__ECDSA_PRIVATE_KEY=" "$S2E_ENV_PATH" | cut -d'=' -f2 | tr -d '[:space:]')
if [ -z "$S2E_PRIVATE_KEY" ]; then
    echo -e "${RED}❌ 错误: 未找到 RELAYER__ECDSA_PRIVATE_KEY 配置${NC}"
    exit 1
fi

# 从私钥推导地址
echo "  正在从私钥推导 S2E Relayer 地址..."
S2E_ADDRESS=$(cd "$SCRIPT_DIR" && node -e "
const { ethers } = require('ethers');
const wallet = new ethers.Wallet('$S2E_PRIVATE_KEY');
console.log(wallet.address);
" 2>/dev/null)

if [ -z "$S2E_ADDRESS" ]; then
    echo -e "${RED}❌ 错误: 无法从私钥推导地址${NC}"
    exit 1
fi
echo -e "${GREEN}✓ S2E Relayer 地址: ${CYAN}${S2E_ADDRESS}${NC}"

# E2S Relayer (SVM)
E2S_ENV_PATH="$PROJECT_ROOT/relayer/e2s-submitter/.env"
if [ ! -f "$E2S_ENV_PATH" ]; then
    echo -e "${RED}❌ 错误: 未找到 E2S Relayer 配置文件: $E2S_ENV_PATH${NC}"
    echo -e "${YELLOW}提示: 请先运行 04-register-relayer.sh 注册 Relayer${NC}"
    exit 1
fi

E2S_PRIVATE_KEY=$(grep "^RELAYER__ED25519_PRIVATE_KEY=" "$E2S_ENV_PATH" | cut -d'=' -f2 | tr -d '[:space:]')
if [ -z "$E2S_PRIVATE_KEY" ]; then
    echo -e "${RED}❌ 错误: 未找到 RELAYER__ED25519_PRIVATE_KEY 配置${NC}"
    exit 1
fi

# 从私钥推导公钥（Ed25519 格式）
echo "  正在从私钥推导 E2S Relayer 公钥..."
E2S_KEYPAIR_PATH="$PROJECT_ROOT/.relayer/e2s-relayer-keypair.json"
if [ ! -f "$E2S_KEYPAIR_PATH" ]; then
    echo -e "${RED}❌ 错误: 未找到 E2S Relayer 密钥文件: $E2S_KEYPAIR_PATH${NC}"
    echo -e "${YELLOW}提示: 请先运行 04-register-relayer.sh 注册 Relayer${NC}"
    exit 1
fi

E2S_PUBKEY=$(solana-keygen pubkey "$E2S_KEYPAIR_PATH" 2>/dev/null)
if [ -z "$E2S_PUBKEY" ]; then
    echo -e "${RED}❌ 错误: 无法从密钥文件读取公钥${NC}"
    exit 1
fi
echo -e "${GREEN}✓ E2S Relayer 公钥: ${CYAN}${E2S_PUBKEY}${NC}"
echo ""

# ==============================================================================
# EVM: 转账 ETH 到 S2E Relayer
# ==============================================================================

echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}  EVM: 充值 S2E Relayer${NC}"
echo -e "${CYAN}========================================${NC}"
echo ""

# 读取 EVM 配置
EVM_RPC_URL="${EVM_RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}"
if [[ ! "$EVM_RPC_URL" =~ ^https?:// ]]; then
    EVM_RPC_URL="https://$EVM_RPC_URL"
fi

ADMIN_EVM_PRIVATE_KEY="${ADMIN_EVM_PRIVATE_KEY}"
if [ -z "$ADMIN_EVM_PRIVATE_KEY" ]; then
    echo -e "${RED}❌ 错误: 未找到 ADMIN_EVM_PRIVATE_KEY 配置${NC}"
    exit 1
fi

TRANSFER_AMOUNT="0.001"  # ETH
TRANSFER_AMOUNT_WEI=$(cd "$SCRIPT_DIR" && node -e "
const { ethers } = require('ethers');
console.log(ethers.parseEther('$TRANSFER_AMOUNT').toString());
" 2>/dev/null)

echo -e "${BLUE}步骤 3/4: 从 EVM 管理员账户转账到 S2E Relayer...${NC}"
echo "  说明: 为 S2E Relayer 充值 ETH，用于支付 EVM 链上的 gas 费用"
echo "  管理员地址: $(cd "$SCRIPT_DIR" && node -e "const { ethers } = require('ethers'); const wallet = new ethers.Wallet('$ADMIN_EVM_PRIVATE_KEY'); console.log(wallet.address);")"
echo "  Relayer 地址: $S2E_ADDRESS"
echo "  转账金额: $TRANSFER_AMOUNT ETH"
echo ""

# 检查管理员余额
echo "  检查管理员账户余额..."
ADMIN_BALANCE=$(cd "$SCRIPT_DIR" && node -e "
const { ethers } = require('ethers');
const provider = new ethers.JsonRpcProvider('$EVM_RPC_URL');
const adminWallet = new ethers.Wallet('$ADMIN_EVM_PRIVATE_KEY', provider);
provider.getBalance(adminWallet.address).then(balance => {
    const ethBalance = ethers.formatEther(balance);
    console.log(ethBalance);
}).catch(err => {
    console.error('Error:', err.message);
    process.exit(1);
});
" 2>&1)

if [ $? -ne 0 ]; then
    echo -e "${RED}❌ 错误: 无法查询管理员余额${NC}"
    exit 1
fi

echo "  管理员余额: $ADMIN_BALANCE ETH"

# 检查是否需要转账
RELAYER_BALANCE=$(cd "$SCRIPT_DIR" && node -e "
const { ethers } = require('ethers');
const provider = new ethers.JsonRpcProvider('$EVM_RPC_URL');
provider.getBalance('$S2E_ADDRESS').then(balance => {
    const ethBalance = ethers.formatEther(balance);
    console.log(ethBalance);
}).catch(err => {
    console.error('Error:', err.message);
    process.exit(1);
});
" 2>&1)

echo "  Relayer 当前余额: $RELAYER_BALANCE ETH"

# 执行转账
echo ""
echo "  正在执行转账..."
cd "$SCRIPT_DIR"
TRANSFER_RESULT=$(node -e "
const { ethers } = require('ethers');
const provider = new ethers.JsonRpcProvider('$EVM_RPC_URL');
const adminWallet = new ethers.Wallet('$ADMIN_EVM_PRIVATE_KEY', provider);

async function transfer() {
    try {
        const tx = await adminWallet.sendTransaction({
            to: '$S2E_ADDRESS',
            value: '$TRANSFER_AMOUNT_WEI'
        });
        console.log('TX_HASH:' + tx.hash);
        console.log('TX_URL:https://sepolia.arbiscan.io/tx/' + tx.hash);
        const receipt = await tx.wait();
        if (receipt.status === 1) {
            console.log('SUCCESS');
        } else {
            console.log('FAILED');
            process.exit(1);
        }
    } catch (error) {
        console.error('ERROR:' + error.message);
        process.exit(1);
    }
}

transfer();
" 2>&1)

if echo "$TRANSFER_RESULT" | grep -q "SUCCESS"; then
    TX_HASH=$(echo "$TRANSFER_RESULT" | grep "TX_HASH:" | cut -d':' -f2-)
    TX_URL=$(echo "$TRANSFER_RESULT" | grep "TX_URL:" | cut -d':' -f2-)
    echo -e "${GREEN}✓ 转账成功${NC}"
    echo "  交易哈希: $TX_HASH"
    echo "  查看交易: $TX_URL"
else
    echo -e "${RED}❌ 转账失败${NC}"
    echo "$TRANSFER_RESULT"
    exit 1
fi

echo ""

# ==============================================================================
# SVM: 申请 Airdrop 到 E2S Relayer
# ==============================================================================

echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}  SVM: 充值 E2S Relayer${NC}"
echo -e "${CYAN}========================================${NC}"
echo ""

# 读取 SVM 配置
SVM_RPC_URL="${SVM_RPC_URL:-https://rpc-testnet.1024chain.com/rpc/}"
AIRDROP_AMOUNT=2  # SOL

echo -e "${BLUE}步骤 4/4: 为 E2S Relayer 申请 Solana Airdrop...${NC}"
echo "  说明: 为 E2S Relayer 申请 SOL，用于支付 Solana 链上的交易费用"
echo "  Relayer 公钥: $E2S_PUBKEY"
echo "  Airdrop 金额: $AIRDROP_AMOUNT SOL"
echo ""

# 检查当前余额
echo "  检查 Relayer 账户余额..."
CURRENT_BALANCE=$(solana balance --url "$SVM_RPC_URL" "$E2S_PUBKEY" 2>/dev/null | awk '{print $1}' || echo "0")
echo "  当前余额: $CURRENT_BALANCE SOL"

# 申请 airdrop
echo ""
echo "  正在申请 airdrop..."
AIRDROP_OUTPUT=$(solana airdrop "$AIRDROP_AMOUNT" "$E2S_PUBKEY" --url "$SVM_RPC_URL" 2>&1)
AIRDROP_EXIT=$?

if [ $AIRDROP_EXIT -eq 0 ]; then
    echo -e "${GREEN}✓ Airdrop 成功${NC}"
    # 等待确认
    sleep 2
    NEW_BALANCE=$(solana balance --url "$SVM_RPC_URL" "$E2S_PUBKEY" 2>/dev/null | awk '{print $1}' || echo "0")
    echo "  新余额: $NEW_BALANCE SOL"
else
    if echo "$AIRDROP_OUTPUT" | grep -qi "airdrop.*disabled\|airdrop.*not.*available\|not.*testnet"; then
        echo -e "${YELLOW}⚠ 当前网络不支持 airdrop${NC}"
        echo "  请手动充值: $E2S_PUBKEY"
    else
        echo -e "${YELLOW}⚠ Airdrop 可能失败，但继续执行...${NC}"
        echo "  输出: $AIRDROP_OUTPUT"
    fi
fi

echo ""

# ==============================================================================
# 完成总结
# ==============================================================================

echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Relayer 账户充值完成！${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo -e "${CYAN}充值结果:${NC}"
echo ""
echo -e "${BLUE}E2S Relayer (Solana):${NC}"
echo "  公钥: ${CYAN}${E2S_PUBKEY}${NC}"
FINAL_E2S_BALANCE=$(solana balance --url "$SVM_RPC_URL" "$E2S_PUBKEY" 2>/dev/null | awk '{print $1}' || echo "未知")
echo "  余额: $FINAL_E2S_BALANCE SOL"
echo ""
echo -e "${BLUE}S2E Relayer (EVM):${NC}"
echo "  地址: ${CYAN}${S2E_ADDRESS}${NC}"
FINAL_S2E_BALANCE=$(cd "$SCRIPT_DIR" && node -e "
const { ethers } = require('ethers');
const provider = new ethers.JsonRpcProvider('$EVM_RPC_URL');
provider.getBalance('$S2E_ADDRESS').then(balance => {
    const ethBalance = ethers.formatEther(balance);
    console.log(ethBalance + ' ETH');
}).catch(() => console.log('未知'));
" 2>&1 | tail -1)
echo "  余额: $FINAL_S2E_BALANCE"
echo ""
echo -e "${YELLOW}下一步操作:${NC}"
echo "  启动 Relayer 服务: cd relayer && docker-compose up"
echo ""

