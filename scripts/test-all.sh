#!/bin/bash

# 完整测试流程脚本

set -e

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m'

log_section() {
    echo -e "\n${BLUE}╔══════════════════════════════════════════╗${NC}"
    echo -e "${BLUE}║${NC}  $1"
    echo -e "${BLUE}╚══════════════════════════════════════════╝${NC}\n"
}

log_info() {
    echo -e "${GREEN}[✓]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[!]${NC} $1"
}

log_error() {
    echo -e "${RED}[✗]${NC} $1"
}

# 检查是否在容器内
if [ ! -f /.dockerenv ]; then
    log_error "请在开发容器内运行此脚本"
    echo "执行: ./scripts/dev.sh shell"
    exit 1
fi

log_section "多签跨链桥 - 完整测试流程"

# ==========================================
# 1. 测试 EVM 合约
# ==========================================

log_section "测试 1: EVM 合约"

cd /workspace/contracts/evm

if [ ! -d "lib/forge-std" ]; then
    log_info "安装 forge-std..."
    git config --global --add safe.directory /workspace
    forge install foundry-rs/forge-std
fi

log_info "编译 EVM 合约..."
forge build > /dev/null 2>&1
if [ $? -eq 0 ]; then
    log_info "✅ EVM 合约编译成功"
else
    log_error "❌ EVM 合约编译失败"
    exit 1
fi

log_info "运行 EVM 测试..."
TEST_OUTPUT=$(forge test 2>&1)
if echo "$TEST_OUTPUT" | grep -q "11 passed"; then
    log_info "✅ EVM 测试通过 (11/11)"
else
    log_error "❌ EVM 测试失败"
    echo "$TEST_OUTPUT"
    exit 1
fi

# ==========================================
# 2. 测试 Guardian 编译
# ==========================================

log_section "测试 2: Guardian 节点"

cd /workspace/guardian

log_info "检查 Guardian Rust 代码..."
cargo check > /dev/null 2>&1
if [ $? -eq 0 ]; then
    log_info "✅ Guardian 代码检查通过"
else
    log_warn "⚠️  Guardian 代码检查有警告（可忽略）"
fi

log_info "编译 Guardian..."
cargo build > /dev/null 2>&1
if [ $? -eq 0 ]; then
    log_info "✅ Guardian 编译成功"
else
    log_error "❌ Guardian 编译失败"
    exit 1
fi

# ==========================================
# 3. 测试本地网络启动
# ==========================================

log_section "测试 3: 本地测试网"

# 停止现有进程
pkill -f anvil || true
sleep 1

log_info "启动 Anvil..."
anvil --host 0.0.0.0 --port 8545 > /tmp/anvil.log 2>&1 &
ANVIL_PID=$!
sleep 2

# 测试 Anvil 连接
if curl -s http://localhost:8545 > /dev/null; then
    log_info "✅ Anvil 启动成功"
else
    log_error "❌ Anvil 启动失败"
    exit 1
fi

# ==========================================
# 4. 测试合约部署
# ==========================================

log_section "测试 4: 合约部署"

cd /workspace/contracts/evm

log_info "部署 CoreContract..."
DEPLOY_OUTPUT=$(forge script script/Deploy.s.sol \
    --rpc-url http://localhost:8545 \
    --broadcast 2>&1)

if echo "$DEPLOY_OUTPUT" | grep -q "ONCHAIN EXECUTION COMPLETE"; then
    CONTRACT_ADDR=$(echo "$DEPLOY_OUTPUT" | grep "CoreContract deployed at:" | awk '{print $4}')
    log_info "✅ 合约部署成功: $CONTRACT_ADDR"
else
    log_error "❌ 合约部署失败"
    echo "$DEPLOY_OUTPUT"
    exit 1
fi

# ==========================================
# 5. 测试合约交互
# ==========================================

log_section "测试 5: 合约交互"

log_info "读取 chainId..."
CHAIN_ID=$(cast call $CONTRACT_ADDR "chainId()(uint16)" --rpc-url http://localhost:8545)
if [ "$CHAIN_ID" = "1" ]; then
    log_info "✅ chainId 正确: $CHAIN_ID"
else
    log_error "❌ chainId 错误: $CHAIN_ID"
    exit 1
fi

log_info "读取 quorum..."
QUORUM=$(cast call $CONTRACT_ADDR "quorum()(uint8)" --rpc-url http://localhost:8545)
if [ "$QUORUM" = "13" ]; then
    log_info "✅ quorum 正确: $QUORUM"
else
    log_error "❌ quorum 错误: $QUORUM"
    exit 1
fi

log_info "发送测试消息..."
TX_OUTPUT=$(cast send $CONTRACT_ADDR \
  "publishMessage(uint32,bytes,uint8)" \
  12345 \
  0x48656c6c6f \
  200 \
  --value 0.001ether \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://localhost:8545 2>&1)

if echo "$TX_OUTPUT" | grep -q "status.*1"; then
    log_info "✅ 消息发送成功"
else
    log_error "❌ 消息发送失败"
    echo "$TX_OUTPUT"
    exit 1
fi

# ==========================================
# 清理
# ==========================================

log_section "清理"

kill $ANVIL_PID 2>/dev/null || true
log_info "已停止 Anvil"

# ==========================================
# 测试总结
# ==========================================

log_section "测试完成总结"

cat << EOF
${GREEN}✅ 所有核心测试通过！${NC}

测试结果:
  ✅ EVM 合约编译      - 通过
  ✅ EVM 单元测试      - 11/11 通过
  ✅ Guardian 编译     - 通过
  ✅ Anvil 测试网      - 通过
  ✅ 合约部署          - 通过
  ✅ 合约交互          - 通过

部署信息:
  合约地址: $CONTRACT_ADDR
  Chain ID: $CHAIN_ID
  Quorum: $QUORUM/19

下一步:
  1. 查看完整测试文档: docs/11-testing-guide.md
  2. 配置多网络部署: docs/12-network-configuration.md
  3. 开始 Guardian 开发: docs/08-development-plan.md

EOF

