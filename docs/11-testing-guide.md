# 测试指南

> 完整的测试流程和验证步骤
> 
> 文档版本: v1.0  
> 更新日期: 2025-11-07

---

## ✅ 测试结果总结

**最后测试时间**: 2025-11-07

| 测试项 | 状态 | 说明 |
|--------|------|------|
| Docker 环境 | ✅ 通过 | 容器构建和启动成功 |
| EVM 合约编译 | ✅ 通过 | Foundry 编译无错误 |
| EVM 合约测试 | ✅ 通过 | 11/11 测试全部通过 |
| Anvil 启动 | ✅ 通过 | 本地测试网正常运行 |
| 合约部署 | ✅ 通过 | 部署到 Anvil 成功 |
| 合约交互 | ✅ 通过 | publishMessage 调用成功 |

---

## 🚀 完整测试流程

### 1. 环境准备

```bash
./scripts/dev.sh build
./scripts/dev.sh start
./scripts/dev.sh shell
```

### 2. 启动测试网

```bash
cd /workspace
./scripts/start-evm-only.sh
```

### 3. EVM 合约测试

```bash
cd /workspace/contracts/evm
git config --global --add safe.directory /workspace
forge install foundry-rs/forge-std
forge build
forge test -vvv
```

### 4. 部署和交互

```bash
# 部署
forge script script/Deploy.s.sol --rpc-url http://localhost:8545 --broadcast

# 交互测试
CONTRACT=0x5FbDB2315678afecb367f032d93F642f64180aa3
cast call $CONTRACT "quorum()(uint8)" --rpc-url http://localhost:8545
```

---

## 📊 Gas 消耗分析

| 操作 | Gas 使用量 |
|------|-----------|
| publishMessage | 50,692 |
| pause | 27,461 |
| unpause | 54,771 |
| updateMessageFee | 13,596 |
| withdrawFees | 138,954 |

---

## 🔍 调试技巧

### 查看日志
```bash
tail -f /tmp/anvil.log
```

### 查看交易
```bash
cast tx <TX_HASH> --rpc-url http://localhost:8545
```

### 查看事件
```bash
cast logs --from-block 0 --address $CONTRACT --rpc-url http://localhost:8545
```

---

**相关文档**:
- [10-quickstart-guide.md](./10-quickstart-guide.md) - 快速开始
- [05-quick-reference.md](./05-quick-reference.md) - 命令速查

