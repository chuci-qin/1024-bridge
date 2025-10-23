#!/bin/bash

# Bridge Program 初始化脚本

echo "🔧 Bridge Program 初始化"
echo ""

# 配置
SOLANA_RPC="https://testnet-rpc.1024chain.com/rpc/"
BRIDGE_PROGRAM="HKiMGJ9E6riEBEg4W6iYzWUhJ6qx1dQkJxJR5Yv4wWq"
USDC_MINT="2AkkM5yhowaoZxQU5rAkyKYjBXYybF2NjrASzdeu7qkW"

echo "📊 配置："
echo "   Solana RPC: $SOLANA_RPC"
echo "   Bridge Program: $BRIDGE_PROGRAM"
echo "   USDC Mint: $USDC_MINT"
echo ""

# 设置Solana CLI使用我们的RPC
solana config set --url $SOLANA_RPC

# 检查余额
echo "💰 检查钱包余额..."
solana balance

echo ""
echo "🔸 步骤1: 调用initialize指令"
echo ""
echo "⚠️  需要使用Anchor CLI或手动构造initialize交易"
echo ""
echo "如果你有Anchor CLI:"
echo "  cd programs/bridge"
echo "  anchor run initialize"
echo ""
echo "或者使用solana CLI手动调用（需要构造指令）"
echo ""

# 派生PDA地址
echo "🔸 步骤2: 派生的PDA地址"
echo ""
echo "Bridge State PDA: Ey7zPBYwFEHZgyHbM9ix5m33eeyBoWXHUkqqy33TrBq"
echo "Bridge Authority PDA: B4kc51kqQWrKTPbfnFHZr5WkER5hkt7JUTFMMFWq3RM8"
echo ""

echo "🔸 步骤3: 设置USDC Mint Authority（关键！）"
echo ""
echo "运行以下命令："
echo ""
echo "spl-token authorize $USDC_MINT mint B4kc51kqQWrKTPbfnFHZr5WkER5hkt7JUTFMMFWq3RM8"
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "📝 初始化完成后，Relayer的mint操作才能成功！"

