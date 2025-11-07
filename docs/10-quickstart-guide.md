# 快速开始指南

> 5分钟内运行和测试多签跨链桥项目
> 
> 文档版本: v1.0  
> 更新日期: 2025-11-07

---

## ✅ 已验证的工作流程

**状态**: EVM 部分完全可用  
**测试环境**: Docker 容器 (Ubuntu 24.04)

---

## 📦 1. 启动开发环境

```bash
# 从宿主机执行
./scripts/dev.sh build    # 首次构建（约5-10分钟）
./scripts/dev.sh start     # 启动容器
./scripts/dev.sh shell     # 进入容器
```

---

## 🔧 2. 启动 EVM 测试网

```bash
# 在容器内执行
cd /workspace
./scripts/start-evm-only.sh
```

**预期输出**:
```
✅ Anvil is running on http://localhost:8545
Chain ID: 1337
Default Account: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
```

---

## 🧪 3. 测试 EVM 合约

```bash
cd /workspace/contracts/evm

# 安装依赖（仅首次）
git config --global --add safe.directory /workspace
forge install foundry-rs/forge-std

# 编译和测试
forge build
forge test
```

**预期结果**: ✅ 11/11 tests passed

---

## 🚀 4. 部署合约

```bash
cd /workspace/contracts/evm
forge script script/Deploy.s.sol --rpc-url http://localhost:8545 --broadcast
```

**预期输出**:
```
CoreContract deployed at: 0x5FbDB2315678afecb367f032d93F642f64180aa3
Guardian Set Size: 19
Quorum: 13
```

---

## 📡 5. 与合约交互

```bash
CONTRACT=0x5FbDB2315678afecb367f032d93F642f64180aa3

# 读取配置
cast call $CONTRACT "chainId()(uint16)" --rpc-url http://localhost:8545

# 发送跨链消息
cast send $CONTRACT \
  "publishMessage(uint32,bytes,uint8)" \
  12345 0x48656c6c6f 200 \
  --value 0.001ether \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://localhost:8545
```

---

## 🐛 常见问题

### forge-std 找不到
```bash
cd /workspace/contracts/evm
forge install foundry-rs/forge-std
```

### 端口被占用
```bash
pkill -f anvil
```

---

**相关文档**: 
- [11-testing-guide.md](./11-testing-guide.md) - 完整测试指南
- [12-network-configuration.md](./12-network-configuration.md) - 网络配置

