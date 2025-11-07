# EVM Core Contract

基于 Foundry 的 EVM 链核心桥接合约。

## 快速开始

### 安装依赖

```bash
forge install foundry-rs/forge-std
```

### 编译

```bash
forge build
```

### 测试

```bash
# 运行所有测试
forge test

# 详细输出
forge test -vvv

# Gas 报告
forge test --gas-report
```

### 部署

```bash
# 部署到本地 Anvil
forge script script/Deploy.s.sol:DeployScript \
  --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast
```

## 合约功能

### CoreContract

**核心功能**:
- ✅ 发布跨链消息 (`publishMessage`)
- ✅ Guardian Set 管理
- ✅ 序列号追踪
- ✅ 暂停机制
- ✅ 手续费管理

**事件**:
- `LogMessagePublished` - 消息发布事件 (Guardian 监听)
- `GuardianSetAdded` - Guardian Set 添加
- `GuardianSetUpdated` - Guardian Set 更新
- `ContractPaused` - 合约暂停状态变更

## 开发

### 项目结构

```
contracts/evm/
├── src/
│   └── CoreContract.sol       # 核心合约
├── test/
│   └── CoreContract.t.sol     # 测试文件
├── script/
│   └── Deploy.s.sol           # 部署脚本
└── foundry.toml               # Foundry 配置
```

### 测试覆盖率

```bash
forge coverage
```

### 代码格式化

```bash
forge fmt
```

## 下一步

- [ ] 实现 VAA 验证逻辑
- [ ] 添加 Guardian Set 更新功能
- [ ] 实现 Token Vault
- [ ] 添加更多测试用例

