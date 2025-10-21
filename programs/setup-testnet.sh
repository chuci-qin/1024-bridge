#!/bin/bash

# 1024Chain Testnet 设置脚本
# 用途：创建USDC Token并部署Bridge Program

set -e  # 遇到错误立即退出

echo "🚀 开始设置1024Chain Testnet Bridge..."
echo ""

# ========================================
# 第1步：配置RPC
# ========================================

echo "📡 第1步：配置Solana CLI..."
solana config set --url https://testnet-rpc.1024chain.com/rpc/

echo "✅ RPC已配置"
echo ""

# 检查余额
echo "💰 检查钱包余额..."
BALANCE=$(solana balance | awk '{print $1}')
echo "   当前余额: $BALANCE SOL"

if (( $(echo "$BALANCE < 1" | bc -l) )); then
    echo "⚠️  余额不足，需要至少1 SOL"
    echo "   请从faucet获取测试SOL，然后重新运行此脚本"
    exit 1
fi

echo ""

# ========================================
# 第2步：创建USDC Token
# ========================================

echo "🪙 第2步：创建USDC Token..."

# 检查是否已有USDC（从文件读取）
if [ -f ".usdc-mint" ]; then
    USDC_MINT=$(cat .usdc-mint)
    echo "ℹ️  找到已存在的USDC Mint: $USDC_MINT"
    echo "   是否要创建新的？(y/N)"
    read -r response
    if [[ "$response" != "y" && "$response" != "Y" ]]; then
        echo "   使用现有USDC: $USDC_MINT"
    else
        echo "   创建新USDC Token..."
        USDC_MINT=$(spl-token create-token --decimals 6 | grep "Creating token" | awk '{print $3}')
        echo "$USDC_MINT" > .usdc-mint
        echo "✅ USDC Mint创建成功: $USDC_MINT"
    fi
else
    echo "   创建USDC Token（6位小数）..."
    USDC_MINT=$(spl-token create-token --decimals 6 | grep "Creating token" | awk '{print $3}')
    
    if [ -z "$USDC_MINT" ]; then
        echo "❌ USDC创建失败"
        exit 1
    fi
    
    echo "$USDC_MINT" > .usdc-mint
    echo "✅ USDC Mint创建成功: $USDC_MINT"
fi

echo ""

# 创建USDC账户（如果没有）
echo "📝 创建USDC Token账户..."
spl-token create-account $USDC_MINT 2>/dev/null || echo "   账户已存在"

# 铸造测试USDC
echo "💰 铸造测试USDC..."
spl-token mint $USDC_MINT 10000000000  # 10,000 USDC

echo "✅ 测试USDC已铸造"
echo ""

# ========================================
# 第3步：编译Program
# ========================================

echo "🔨 第3步：编译Bridge Program..."

cd bridge
cargo build-sbf

if [ ! -f "target/deploy/bridge.so" ]; then
    echo "❌ 编译失败：找不到bridge.so"
    exit 1
fi

echo "✅ 编译成功: target/deploy/bridge.so"
cd ..
echo ""

# ========================================
# 第4步：部署Program
# ========================================

echo "🚀 第4步：部署Bridge Program..."

# 部署
PROGRAM_OUTPUT=$(solana program deploy bridge/target/deploy/bridge.so)
PROGRAM_ID=$(echo "$PROGRAM_OUTPUT" | grep "Program Id:" | awk '{print $3}')

if [ -z "$PROGRAM_ID" ]; then
    echo "❌ 部署失败"
    exit 1
fi

echo "$PROGRAM_ID" > .program-id
echo "✅ Program部署成功: $PROGRAM_ID"
echo ""

# ========================================
# 第5步：设置Mint Authority
# ========================================

echo "🔐 第5步：设置Bridge为USDC Mint Authority..."

# 计算Bridge Authority PDA
# 注意：这需要和程序代码中的seeds一致
# seeds = [b"bridge-authority"]

echo "⚠️  关键步骤：需要手动设置mint authority"
echo ""
echo "运行以下命令："
echo "spl-token authorize $USDC_MINT mint <BRIDGE_AUTHORITY_PDA>"
echo ""
echo "Bridge Authority PDA需要从程序派生"
echo "或者暂时保持当前钱包为mint authority（测试阶段）"
echo ""

# ========================================
# 完成
# ========================================

echo "✅ 设置完成！"
echo ""
echo "📋 重要信息："
echo "   USDC Mint: $USDC_MINT"
echo "   Program ID: $PROGRAM_ID"
echo ""
echo "💾 信息已保存到："
echo "   .usdc-mint - USDC Mint地址"
echo "   .program-id - Program ID"
echo ""
echo "🔗 查看Token:"
echo "   solana-explorer: https://explorer.solana.com/address/$USDC_MINT?cluster=custom&customUrl=https://testnet-rpc.1024chain.com/rpc/"
echo ""
echo "🔗 查看Program:"
echo "   solana-explorer: https://explorer.solana.com/address/$PROGRAM_ID?cluster=custom&customUrl=https://testnet-rpc.1024chain.com/rpc/"
echo ""
echo "📝 下一步："
echo "   1. 更新Anchor.toml中的program ID"
echo "   2. 更新lib.rs中的declare_id!"
echo "   3. 运行测试: anchor test"
echo ""

