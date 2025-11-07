#!/bin/bash

# 多网络部署脚本
# 用法: ./scripts/deploy.sh [network] [component]
# 例如: ./scripts/deploy.sh testnet evm
#       ./scripts/deploy.sh local all

set -e

# 颜色输出
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_section() {
    echo -e "${BLUE}[====]${NC} $1"
}

# 默认参数
NETWORK=${1:-local}
COMPONENT=${2:-all}

log_section "多签跨链桥部署脚本"
echo "网络: $NETWORK"
echo "组件: $COMPONENT"
echo ""

# 加载网络配置
CONFIG_FILE="config/networks.toml"
if [ ! -f "$CONFIG_FILE" ]; then
    log_error "配置文件不存在: $CONFIG_FILE"
    exit 1
fi

# 解析配置（简化版，实际可用 toml 解析工具）
get_config() {
    local network=$1
    local chain=$2
    local key=$3
    
    # 使用 grep 和 sed 简单解析
    grep -A 20 "^\[$network.$chain\]" "$CONFIG_FILE" | grep "^$key" | cut -d'"' -f2
}

# 部署 EVM 合约
deploy_evm() {
    local network=$1
    
    log_section "部署 EVM 合约到 $network"
    
    # 获取配置
    local rpc_url=$(get_config "$network" "evm" "rpc_url")
    local chain_id=$(grep -A 20 "^\[$network.evm\]" "$CONFIG_FILE" | grep "^chain_id" | cut -d'=' -f2 | tr -d ' ')
    
    log_info "RPC URL: $rpc_url"
    log_info "Chain ID: $chain_id"
    
    # 检查私钥
    if [ "$network" = "local" ]; then
        # 本地使用 Anvil 默认密钥
        PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
    else
        # 从环境变量读取
        env_var="${network^^}_DEPLOYER_PRIVATE_KEY"
        PRIVATE_KEY="${!env_var}"
        
        if [ -z "$PRIVATE_KEY" ]; then
            log_error "请设置环境变量: $env_var"
            exit 1
        fi
    fi
    
    # 部署合约
    cd /workspace/contracts/evm
    
    log_info "编译合约..."
    forge build
    
    log_info "部署到 $network..."
    forge script script/Deploy.s.sol \
        --rpc-url "$rpc_url" \
        --private-key "$PRIVATE_KEY" \
        --broadcast \
        --verify || {
            log_warn "部署完成但验证失败（可能需要 API key）"
        }
    
    log_info "✅ EVM 合约部署完成"
}

# 部署 Solana 程序
deploy_solana() {
    local network=$1
    
    log_section "部署 Solana 程序到 $network"
    
    # 获取配置
    local rpc_url=$(get_config "$network" "solana" "rpc_url")
    
    log_info "RPC URL: $rpc_url"
    
    # 配置 Solana CLI
    solana config set --url "$rpc_url"
    
    # 检查钱包
    if [ ! -f ~/.config/solana/id.json ]; then
        log_warn "Solana 钱包不存在，创建新钱包..."
        solana-keygen new --no-bip39-passphrase
    fi
    
    local wallet=$(solana address)
    log_info "钱包地址: $wallet"
    
    # 检查余额
    local balance=$(solana balance 2>/dev/null || echo "0")
    log_info "当前余额: $balance"
    
    if [ "$network" = "local" ]; then
        log_info "空投 SOL 到钱包..."
        solana airdrop 10 || log_warn "空投失败（可能已有足够余额）"
    elif [ "$network" = "testnet" ]; then
        log_warn "请确保钱包有足够的 SOL，可通过水龙头获取:"
        log_warn "https://faucet.solana.com/"
        read -p "按回车继续..."
    fi
    
    # 部署程序
    cd /workspace/programs/solana-core
    
    log_info "构建程序..."
    anchor build
    
    log_info "部署程序..."
    anchor deploy --provider.cluster $([ "$network" = "local" ] && echo "localnet" || echo "devnet")
    
    log_info "✅ Solana 程序部署完成"
}

# 主逻辑
case "$COMPONENT" in
    evm)
        deploy_evm "$NETWORK"
        ;;
    solana)
        deploy_solana "$NETWORK"
        ;;
    all)
        deploy_evm "$NETWORK"
        echo ""
        deploy_solana "$NETWORK"
        ;;
    *)
        log_error "未知组件: $COMPONENT"
        echo "用法: $0 [network] [component]"
        echo ""
        echo "网络选项:"
        echo "  local      - 本地测试网 (Anvil + Solana Test Validator)"
        echo "  testnet    - 公共测试网 (Sepolia + Solana Devnet)"
        echo "  mainnet    - 生产主网 (Ethereum + Solana Mainnet)"
        echo "  bsc_testnet, polygon_mumbai, arbitrum_sepolia 等"
        echo ""
        echo "组件选项:"
        echo "  evm        - 仅部署 EVM 合约"
        echo "  solana     - 仅部署 Solana 程序"
        echo "  all        - 部署所有组件"
        exit 1
        ;;
esac

log_section "部署完成！"

