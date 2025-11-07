#!/bin/bash

# 仅启动 EVM 测试网脚本（用于 Solana 不可用时）

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

# 停止现有的测试网
log_info "Stopping existing testnets..."
pkill -f "anvil" || true
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

# 保存 PID
echo $ANVIL_PID > /tmp/anvil.pid

# 测试默认账户
DEFAULT_PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
DEFAULT_ADDRESS="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

cat << EOF

╔═══════════════════════════════════════════════════════════╗
║              🎉 EVM Testnet Started                       ║
╠═══════════════════════════════════════════════════════════╣
║                                                           ║
║  📡 Anvil (EVM)                                           ║
║     RPC:        http://localhost:8545                     ║
║     Chain ID:   1337                                      ║
║     Log:        /tmp/anvil.log                            ║
║                                                           ║
║  💰 Default Account:                                      ║
║     Address:    $DEFAULT_ADDRESS                          ║
║     Private:    $DEFAULT_PRIVATE_KEY                      ║
║     Balance:    10000 ETH                                 ║
║                                                           ║
╠═══════════════════════════════════════════════════════════╣
║  Next Steps:                                              ║
║    1. Deploy contracts:                                   ║
║       cd /workspace/contracts/evm                         ║
║       forge script script/Deploy.s.sol \\                 ║
║         --rpc-url http://localhost:8545 \\                ║
║         --private-key $DEFAULT_PRIVATE_KEY \\             ║
║         --broadcast                                       ║
║                                                           ║
║    2. Run tests:                                          ║
║       forge test -vvv                                     ║
║                                                           ║
║    3. Stop testnet:                                       ║
║       ./scripts/stop-testnet.sh                           ║
╚═══════════════════════════════════════════════════════════╝

EOF

log_info "Anvil is ready for testing!"

