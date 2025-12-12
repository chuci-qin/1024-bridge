#!/bin/bash

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# ==============================================================================
# 配置
# ==============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/.."
CONTRACT_DIR="$PROJECT_ROOT/svm/bridge1024"
KEYPAIR_PATH="$CONTRACT_DIR/target/deploy/bridge1024-keypair.json"
PROGRAM_PATH="$CONTRACT_DIR/target/deploy/bridge1024.so"
SVM_CONFIG_FILE="$PROJECT_ROOT/.env.svm.deploy"

# 加载配置
source "$SVM_CONFIG_FILE"

RPC_URL="${SVM_RPC_URL:-https://testnet-rpc.1024chain.com/rpc/}"
ADMIN_KEYPAIR="${SVM_KEYPAIR_PATH:-/root/.config/solana/id.json}"
REQUIRED_BALANCE=5

# ==============================================================================
# 检查 Program ID 一致性
# ==============================================================================

# 如果 keypair 文件存在，检查它与配置文件中的 Program ID 是否一致
if [ -f "$KEYPAIR_PATH" ] && [ -n "$SVM_PROGRAM_ID" ]; then
    KEYPAIR_PROGRAM_ID=$(solana address -k "$KEYPAIR_PATH" 2>/dev/null || echo "")
    if [ -n "$KEYPAIR_PROGRAM_ID" ] && [ "$KEYPAIR_PROGRAM_ID" != "$SVM_PROGRAM_ID" ]; then
        echo -e "${YELLOW}========================================${NC}"
        echo -e "${YELLOW}⚠ Program ID 不一致警告${NC}"
        echo -e "${YELLOW}========================================${NC}"
        echo ""
        echo "检测到配置不一致："
        echo "  配置文件 (.env.svm.deploy): $SVM_PROGRAM_ID"
        echo "  Keypair 文件:                $KEYPAIR_PROGRAM_ID"
        echo ""
        echo -e "${YELLOW}这可能导致合约调用失败！${NC}"
        echo ""
        echo "建议操作："
        echo "  1) 删除 keypair 文件，重新生成（选项2）"
        echo "  2) 继续使用现有 keypair（会自动更新配置）"
        echo ""
        read -p "是否继续？[y/N] " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            echo -e "${RED}操作已取消${NC}"
            exit 1
        fi
        echo -e "${GREEN}将在部署后自动同步所有配置...${NC}"
        echo ""
    fi
fi

# ==============================================================================
# 检查依赖
# ==============================================================================

if ! command -v anchor &> /dev/null; then
    echo -e "${RED}错误: 未安装 Anchor CLI${NC}"
    exit 1
fi

if ! command -v solana &> /dev/null; then
    echo -e "${RED}错误: 未安装 Solana CLI${NC}"
    exit 1
fi

# ==============================================================================
# 检查管理员账户
# ==============================================================================

if [ ! -f "$ADMIN_KEYPAIR" ]; then
    mkdir -p "$(dirname "$ADMIN_KEYPAIR")"
    solana-keygen new --no-bip39-passphrase --outfile "$ADMIN_KEYPAIR" --force > /dev/null 2>&1
    if [ $? -ne 0 ]; then
        echo -e "${RED}错误: 无法生成管理员密钥${NC}"
        exit 1
    fi
fi

ADMIN_ADDRESS=$(solana address -k "$ADMIN_KEYPAIR" 2>/dev/null)
if [ -z "$ADMIN_ADDRESS" ]; then
    echo -e "${RED}错误: 无法获取管理员地址${NC}"
    exit 1
fi

# ==============================================================================
# 检查余额并自动充值
# ==============================================================================

BALANCE=$(solana balance --url "$RPC_URL" -k "$ADMIN_KEYPAIR" 2>/dev/null | awk '{print $1}')

if [ -z "$BALANCE" ]; then
    echo -e "${RED}错误: 无法获取账户余额${NC}"
    exit 1
fi

if (( $(echo "$BALANCE < $REQUIRED_BALANCE" | bc -l) )); then
    MAX_ATTEMPTS=3
    ATTEMPT=0
    
    while (( $(echo "$BALANCE < $REQUIRED_BALANCE" | bc -l) )) && [ $ATTEMPT -lt $MAX_ATTEMPTS ]; do
        ATTEMPT=$((ATTEMPT + 1))
        
        AIRDROP_OUTPUT=$(solana airdrop 2 "$ADMIN_ADDRESS" --url "$RPC_URL" 2>&1)
        AIRDROP_EXIT=$?
        
        if [ $AIRDROP_EXIT -ne 0 ]; then
            if echo "$AIRDROP_OUTPUT" | grep -qi "airdrop.*disabled\|airdrop.*not.*available\|not.*testnet"; then
                echo -e "${RED}错误: 当前网络不支持空投${NC}"
                echo "请手动充值: $ADMIN_ADDRESS"
                exit 1
            fi
            
            if [ $ATTEMPT -ge $MAX_ATTEMPTS ]; then
                echo -e "${RED}错误: 空投失败，余额不足${NC}"
                echo "请手动充值: $ADMIN_ADDRESS"
                exit 1
            fi
            
            sleep 5
        else
            sleep 2
            BALANCE=$(solana balance --url "$RPC_URL" -k "$ADMIN_KEYPAIR" 2>/dev/null | awk '{print $1}')
        fi
    done
fi

# ==============================================================================
# 检查并处理程序密钥对
# ==============================================================================

NEED_UPDATE_DECLARE_ID=false

# 检查是否已存在程序密钥对
if [ -f "$KEYPAIR_PATH" ]; then
    OLD_PROGRAM_ID=$(solana address -k "$KEYPAIR_PATH" 2>/dev/null || echo "unknown")
    echo -e "${YELLOW}发现已存在的程序密钥对${NC}"
    echo "  当前程序ID: $OLD_PROGRAM_ID"
    echo ""
    echo "选择操作："
    echo "  1) 使用现有程序ID（升级部署）"
    echo "  2) 生成新的程序ID（全新部署）"
    echo ""
    read -p "请选择 [1/2] (默认: 1): " DEPLOY_CHOICE
    DEPLOY_CHOICE=${DEPLOY_CHOICE:-1}
    
    if [ "$DEPLOY_CHOICE" = "2" ]; then
        # 备份旧密钥对
        BACKUP_DIR="$PROJECT_ROOT/.backup_$(date +%Y%m%d_%H%M%S)"
        mkdir -p "$BACKUP_DIR"
        cp "$KEYPAIR_PATH" "$BACKUP_DIR/old-keypair.json"
        echo -e "${GREEN}✓ 已备份旧密钥对到: $BACKUP_DIR${NC}"
        
        # 生成新的程序密钥对
        echo -e "${YELLOW}生成新的程序密钥对...${NC}"
        solana-keygen new --no-bip39-passphrase --outfile "$KEYPAIR_PATH" --force > /dev/null 2>&1
        NEW_PROGRAM_ID=$(solana address -k "$KEYPAIR_PATH")
        echo -e "${GREEN}✓ 新程序ID: $NEW_PROGRAM_ID${NC}"
        echo ""
        NEED_UPDATE_DECLARE_ID=true
    else
        echo -e "${GREEN}✓ 将使用现有程序ID进行升级部署${NC}"
        echo ""
    fi
else
    echo -e "${YELLOW}未找到程序密钥对，将生成新的${NC}"
    mkdir -p "$(dirname "$KEYPAIR_PATH")"
    solana-keygen new --no-bip39-passphrase --outfile "$KEYPAIR_PATH" > /dev/null 2>&1
    NEW_PROGRAM_ID=$(solana address -k "$KEYPAIR_PATH")
    echo -e "${GREEN}✓ 新程序ID: $NEW_PROGRAM_ID${NC}"
    echo ""
    NEED_UPDATE_DECLARE_ID=true
fi

# 获取当前 Program ID
CURRENT_PROGRAM_ID=$(solana address -k "$KEYPAIR_PATH" 2>/dev/null)

# ==============================================================================
# 检查并更新 declare_id!
# ==============================================================================

cd "$CONTRACT_DIR" || exit 1

LIB_RS_PATH="$CONTRACT_DIR/programs/bridge1024/src/lib.rs"

# 从 lib.rs 中提取当前的 declare_id
if [ -f "$LIB_RS_PATH" ]; then
    DECLARED_ID=$(grep -oP 'declare_id!\("\K[^"]+' "$LIB_RS_PATH" 2>/dev/null || echo "")
    
    if [ -n "$DECLARED_ID" ] && [ "$DECLARED_ID" != "$CURRENT_PROGRAM_ID" ]; then
        echo -e "${YELLOW}检测到 declare_id! 与 keypair 不匹配${NC}"
        echo "  lib.rs 中: $DECLARED_ID"
        echo "  keypair:  $CURRENT_PROGRAM_ID"
        NEED_UPDATE_DECLARE_ID=true
    fi
fi

if [ "$NEED_UPDATE_DECLARE_ID" = true ]; then
    echo ""
    echo -e "${YELLOW}更新 Rust 代码中的 declare_id!...${NC}"
    
    if [ -f "$LIB_RS_PATH" ]; then
        # 使用 sed 更新 declare_id!（macOS 兼容）
        sed -i '' "s/declare_id!(\"[^\"]*\")/declare_id!(\"$CURRENT_PROGRAM_ID\")/" "$LIB_RS_PATH"
        echo -e "${GREEN}✓ 已更新 declare_id! 为: $CURRENT_PROGRAM_ID${NC}"
    else
        echo -e "${RED}错误: 找不到 lib.rs 文件${NC}"
        exit 1
    fi
fi

# ==============================================================================
# 编译合约
# ==============================================================================

echo ""
echo "编译合约..."
if ! anchor build > /dev/null 2>&1; then
    echo -e "${RED}错误: 合约编译失败${NC}"
    exit 1
fi
echo -e "${GREEN}✓ 编译成功${NC}"

if [ ! -f "$PROGRAM_PATH" ] || [ ! -f "$KEYPAIR_PATH" ]; then
    echo -e "${RED}错误: 找不到编译产物${NC}"
    exit 1
fi

# ==============================================================================
# 部署合约
# ==============================================================================

PROGRAM_ID=$(solana address -k "$KEYPAIR_PATH" 2>/dev/null)
if [ -z "$PROGRAM_ID" ]; then
    echo -e "${RED}错误: 无法获取程序 ID${NC}"
    exit 1
fi

echo "部署合约到链上..."
echo "  程序ID: $PROGRAM_ID"
echo "  这可能需要几分钟..."

DEPLOY_OUTPUT=$(solana program deploy \
    --url "$RPC_URL" \
    --program-id "$KEYPAIR_PATH" \
    --skip-preflight \
    --max-sign-attempts 10 \
    --use-rpc \
    "$PROGRAM_PATH" 2>&1)

if [ $? -ne 0 ]; then
    echo -e "${RED}错误: 合约部署失败${NC}"
    echo "$DEPLOY_OUTPUT"
    exit 1
fi

echo -e "${GREEN}✓ 部署成功${NC}"

# ==============================================================================
# 初始化合约
# ==============================================================================

echo ""
echo "初始化合约..."
echo "  程序ID: $PROGRAM_ID"
echo "  管理员: $ADMIN_ADDRESS"
echo ""

# 切换到 scripts 目录执行初始化
cd "$SCRIPT_DIR" || exit 1

# 设置环境变量供 svm-admin.ts 使用
export SVM_RPC_URL="$RPC_URL"
export SVM_PROGRAM_ID="$PROGRAM_ID"
export ADMIN_SVM_KEYPAIR_PATH="$ADMIN_KEYPAIR"

# 调用 initialize - 显示实时输出
if timeout 5 ./node_modules/.bin/ts-node svm-admin.ts initialize; then
    echo ""
    echo -e "${GREEN}✓ 合约初始化成功${NC}"
else
    INIT_EXIT=$?
    echo ""
    echo -e "${RED}错误: 合约初始化失败（退出码: $INIT_EXIT）${NC}"
    echo -e "${YELLOW}提示: 如果错误是账户已存在 (0x0)，可能是合约已经初始化过了，也可能是ws接口超时 ${NC}"
    echo -e "${YELLOW}      你可以继续下一步，或者使用选项2生成新的程序ID重新部署${NC}"
    # exit 1
fi

echo ""

# 回到合约目录
cd "$CONTRACT_DIR" || exit 1

# ==============================================================================
# 更新配置文件
# ==============================================================================

echo "更新配置文件..."

# 检查配置文件中的 Program ID 是否与 keypair 一致
if [ -f "$SVM_CONFIG_FILE" ] && grep -q "^SVM_PROGRAM_ID=" "$SVM_CONFIG_FILE"; then
    CONFIG_PROGRAM_ID=$(grep "^SVM_PROGRAM_ID=" "$SVM_CONFIG_FILE" | cut -d'=' -f2)
    if [ -n "$CONFIG_PROGRAM_ID" ] && [ "$CONFIG_PROGRAM_ID" != "$PROGRAM_ID" ]; then
        echo -e "${YELLOW}ℹ 检测到配置文件中的 Program ID 与新的 keypair 不一致（这是正常的，如果选择了生成新程序ID）${NC}"
        echo "  旧配置: $CONFIG_PROGRAM_ID"
        echo "  新程序ID: $PROGRAM_ID"
        echo -e "${GREEN}  正在自动更新配置文件...${NC}"
    fi
    sed -i '' "s|^SVM_PROGRAM_ID=.*|SVM_PROGRAM_ID=${PROGRAM_ID}|g" "$SVM_CONFIG_FILE"
else
    echo "SVM_PROGRAM_ID=${PROGRAM_ID}" >> "$SVM_CONFIG_FILE"
fi

if grep -q "^SVM_ADMIN_ADDRESS=" "$SVM_CONFIG_FILE" 2>/dev/null; then
    sed -i '' "s|^SVM_ADMIN_ADDRESS=.*|SVM_ADMIN_ADDRESS=${ADMIN_ADDRESS}|g" "$SVM_CONFIG_FILE"
else
    echo "SVM_ADMIN_ADDRESS=${ADMIN_ADDRESS}" >> "$SVM_CONFIG_FILE"
fi

# 同时更新 .env.invoke（如果存在）
INVOKE_ENV="$PROJECT_ROOT/.env.invoke"
if [ -f "$INVOKE_ENV" ]; then
    if grep -q "^SVM_PROGRAM_ID=" "$INVOKE_ENV"; then
        sed -i '' "s|^SVM_PROGRAM_ID=.*|SVM_PROGRAM_ID=${PROGRAM_ID}|g" "$INVOKE_ENV"
    else
        echo "SVM_PROGRAM_ID=${PROGRAM_ID}" >> "$INVOKE_ENV"
    fi
fi

echo -e "${GREEN}✓ 配置文件已更新${NC}"

# ==============================================================================
# 全项目同步 Program ID
# ==============================================================================

echo ""
echo -e "${YELLOW}正在全项目替换 Program ID...${NC}"

PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo $CONFIG_PROGRAM_ID
echo $PROGRAM_ID
echo $PROJECT_ROOT

# 设置 LC_ALL=C 避免编码问题（macOS 兼容）
export LC_ALL=C

find "$PROJECT_ROOT" \
    -type f ! -path "*/.git/*" ! -path "*/node_modules/*" ! -path "*/cache/*" ! -path "*/.venv/*" ! \
    -path "*/out/*" ! -path "*/target/*" ! -path "*/.next/*" ! -name "*.log" \
    -exec sed -i '' "s|$CONFIG_PROGRAM_ID|$PROGRAM_ID|g" {} \;

# ==============================================================================
# 验证 Program ID 一致性
# ==============================================================================

echo ""
echo "验证 Program ID 一致性..."

# 从各个源读取 Program ID
KEYPAIR_ID=$(solana address -k "$KEYPAIR_PATH" 2>/dev/null || echo "N/A")
CONFIG_ID=$(grep "^SVM_PROGRAM_ID=" "$SVM_CONFIG_FILE" 2>/dev/null | cut -d'=' -f2 || echo "N/A")
DECLARED_ID=$(grep -oP 'declare_id!\("\K[^"]+' "$LIB_RS_PATH" 2>/dev/null || echo "N/A")
IDL_ID=$(grep -oP '"address"\s*:\s*"\K[^"]+' "$CONTRACT_DIR/target/idl/bridge1024.json" 2>/dev/null | head -1 || echo "N/A")

# 检查一致性
CONSISTENCY_OK=true
if [ "$KEYPAIR_ID" = "N/A" ] || [ "$CONFIG_ID" = "N/A" ] || [ "$DECLARED_ID" = "N/A" ] || [ "$IDL_ID" = "N/A" ]; then
    CONSISTENCY_OK=false
elif [ "$KEYPAIR_ID" != "$CONFIG_ID" ] || [ "$KEYPAIR_ID" != "$DECLARED_ID" ] || [ "$KEYPAIR_ID" != "$IDL_ID" ]; then
    CONSISTENCY_OK=false
fi

if [ "$CONSISTENCY_OK" = true ]; then
    echo -e "${GREEN}✓ 所有 Program ID 已同步一致${NC}"
    echo "  Program ID: ${GREEN}${PROGRAM_ID}${NC}"
else
    echo -e "${RED}✗ Program ID 不一致${NC}"
    echo "  Keypair 文件:     $KEYPAIR_ID"
    echo "  配置文件:         $CONFIG_ID"
    echo "  declare_id!:      $DECLARED_ID"
    echo "  IDL 文件:         $IDL_ID"
    echo ""
    echo -e "${YELLOW}⚠ 请检查上述不一致问题${NC}"
fi

# ==============================================================================
# 输出结果
# ==============================================================================

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  SVM 合约部署完成！${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo "程序信息:"
echo "  程序ID: ${GREEN}${PROGRAM_ID}${NC}"
echo "  管理员: ${ADMIN_ADDRESS}"
echo ""
echo "配置文件:"
echo "  .env.svm.deploy 已更新"
[ -f "$INVOKE_ENV" ] && echo "  .env.invoke 已更新"
[ "$UPDATED_COUNT" -gt 0 ] && echo "  已同步 $UPDATED_COUNT 个文件中的 Program ID"
echo ""
echo "下一步操作:"
echo "  运行: ./03-config-usdc-peer.sh"
echo "  配置 USDC 地址和对端合约"
echo ""
