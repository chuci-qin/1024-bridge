#!/bin/bash

# 启动本地测试网络脚本

set -e

# 颜色输出
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
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

# 检查是否在容器内
if [ ! -f /.dockerenv ]; then
    log_error "This script should be run inside the development container"
    log_info "Please run: ./scripts/dev.sh shell"
    exit 1
fi

# 检查 Solana 是否安装
if ! command -v solana &> /dev/null; then
    log_error "Solana CLI is not installed"
    log_info "Installing Solana CLI..."
    
    # 设置临时环境变量
    export PATH="/root/.local/share/solana/install/active_release/bin:$PATH"
    
    # 尝试从已安装位置运行
    if [ -d "/root/.local/share/solana/install/active_release/bin" ]; then
        log_info "Found existing Solana installation, updating PATH"
    else
        log_error "Please install Solana manually:"
        log_info "  curl --proto '=https' --tlsv1.2 -sSf https://release.solana.com/stable/install | sh"
        log_info "  source ~/.bashrc"
        exit 1
    fi
fi

# 停止现有的测试网
log_info "Stopping existing testnets..."
pkill -f "anvil" || true
pkill -f "solana-test-validator" || true
sleep 2

# 启动 EVM 测试网 (Anvil)
log_info "Starting Anvil (EVM testnet)..."
anvil \
    --host 0.0.0.0 \
    --port 8545 \
    --chain-id 1337 \
    --accounts 10 \
    --balance 10000 \
    > /tmp/anvil.log 2>&1 &

ANVIL_PID=$!
log_info "Anvil started with PID: $ANVIL_PID"

# 等待 Anvil 启动
sleep 2

# 测试 Anvil 连接
if curl -s http://localhost:8545 > /dev/null; then
    log_info "✅ Anvil is running on http://localhost:8545"
else
    log_error "❌ Anvil failed to start"
    exit 1
fi

# 启动 Solana 测试网
log_info "Starting Solana Test Validator..."

# 清理旧数据
rm -rf /tmp/test-ledger

solana-test-validator \
    --rpc-port 8899 \
    --faucet-port 9900 \
    --ledger /tmp/test-ledger \
    --reset \
    --quiet \
    > /tmp/solana-validator.log 2>&1 &

SOLANA_PID=$!
log_info "Solana Test Validator started with PID: $SOLANA_PID"

# 等待 Solana 启动
log_info "Waiting for Solana to be ready..."
for i in {1..30}; do
    if solana cluster-version --url http://localhost:8899 > /dev/null 2>&1; then
        log_info "✅ Solana is running on http://localhost:8899"
        break
    fi
    
    if [ $i -eq 30 ]; then
        log_error "❌ Solana failed to start after 30 seconds"
        exit 1
    fi
    
    sleep 1
done

# 配置 Solana CLI
log_info "Configuring Solana CLI..."
solana config set --url http://localhost:8899 > /dev/null

# 创建测试钱包（如果不存在）
if [ ! -f ~/.config/solana/id.json ]; then
    log_info "Creating test wallet..."
    solana-keygen new --no-bip39-passphrase --silent --outfile ~/.config/solana/id.json
fi

# 空投 SOL
WALLET_ADDRESS=$(solana address)
log_info "Airdropping 100 SOL to $WALLET_ADDRESS..."
solana airdrop 100 > /dev/null 2>&1 || true

# 显示余额
BALANCE=$(solana balance 2>/dev/null || echo "0")
log_info "Wallet balance: $BALANCE"

# 保存 PIDs
echo $ANVIL_PID > /tmp/anvil.pid
echo $SOLANA_PID > /tmp/solana.pid

cat << EOF

╔════════════════════════════════════════════════════════════╗
║              🎉 Local Testnets Started                     ║
╠════════════════════════════════════════════════════════════╣
║                                                            ║
║  📡 EVM (Anvil)                                            ║
║     RPC:        http://localhost:8545                      ║
║     Chain ID:   1337                                       ║
║     Log:        /tmp/anvil.log                             ║
║                                                            ║
║  🌐 Solana                                                 ║
║     RPC:        http://localhost:8899                      ║
║     Faucet:     http://localhost:9900                      ║
║     Wallet:     $WALLET_ADDRESS                            ║
║     Balance:    $BALANCE                                   ║
║     Log:        /tmp/solana-validator.log                  ║
║                                                            ║
╠════════════════════════════════════════════════════════════╣
║  Next Steps:                                               ║
║    1. Deploy EVM contracts:  cd contracts/evm && forge... ║
║    2. Deploy Solana program: cd programs/... && anchor... ║
║    3. Stop testnets:         ./scripts/stop-testnet.sh    ║
╚════════════════════════════════════════════════════════════╝

EOF

log_info "Press Ctrl+C to stop testnets, or run ./scripts/stop-testnet.sh"

