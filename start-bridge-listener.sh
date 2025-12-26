#!/bin/bash
# Bridge CrossChainSuccess 监听服务启动脚本

set -e

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$SCRIPT_DIR/.."

echo "🌉 启动 Bridge CrossChainSuccess 监听服务"
echo "========================================"

# 检查环境变量配置文件
ENV_FILE="$SCRIPT_DIR/bridge-listener.env"
if [ ! -f "$ENV_FILE" ]; then
    echo "❌ 未找到配置文件: $ENV_FILE"
    echo ""
    echo "请创建配置文件，示例："
    echo ""
    cat << 'EOF'
# bridge-listener.env
SOLANA_RPC_URL=https://testnet-rpc.1024chain.com/rpc/
BRIDGE_PROGRAM_ID=F7mhpQAE3umJYrBitUHJChiQbEbUFmQRac85uyCW5aKn
VAULT_PROGRAM_ID=vR3BifKCa2TGKP2uhToxZAMYAYydqpesvKGX54gzFny
VAULT_CONFIG_PDA=rMLrkwxV4uNLKmL2vmP3CJbYPbKamjZD4wjeKZsCy1g
RELAY_KEYPAIR_PATH=./relay-transit.json
EOF
    echo ""
    exit 1
fi

# 加载环境变量
echo "📝 加载配置: $ENV_FILE"
export $(grep -v '^#' "$ENV_FILE" | xargs)

# 检查中转账户密钥文件
if [ ! -f "$RELAY_KEYPAIR_PATH" ]; then
    echo "❌ 未找到中转账户密钥文件: $RELAY_KEYPAIR_PATH"
    echo ""
    echo "请生成中转账户密钥："
    echo "  solana-keygen new -o relay-transit.json"
    echo ""
    exit 1
fi

RELAY_PUBKEY=$(solana-keygen pubkey "$RELAY_KEYPAIR_PATH")
echo "✅ 中转账户: $RELAY_PUBKEY"

# 进入 1024-core 目录
cd "$PROJECT_ROOT/1024-core"

echo ""
echo "🚀 启动监听服务..."
echo ""

# 启动服务
cargo run --release --bin bridge-listener


