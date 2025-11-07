# 网络配置指南

> 多网络部署和切换配置
> 
> 文档版本: v1.0  
> 更新日期: 2025-11-07

---

## 📋 支持的网络

### 本地开发网络

| 链 | 网络 | Chain ID | RPC |
|---|------|----------|-----|
| EVM | Anvil | 1337 | http://localhost:8545 |
| Solana | Test Validator | 2 | http://localhost:8899 |

### 公共测试网

| 链 | 网络 | Chain ID | RPC |
|---|------|----------|-----|
| EVM | Sepolia | 11155111 | https://rpc.sepolia.org |
| Solana | Devnet | 2 | https://api.devnet.solana.com |
| BSC | Testnet | 97 | https://data-seed-prebsc-1-s1.binance.org:8545 |
| Polygon | Mumbai | 80001 | https://rpc-mumbai.maticvigil.com |

### 主网

| 链 | 网络 | Chain ID | RPC |
|---|------|----------|-----|
| Ethereum | Mainnet | 1 | https://eth.llamarpc.com |
| Solana | Mainnet | 2 | https://api.mainnet-beta.solana.com |
| BSC | Mainnet | 56 | https://bsc-dataseed1.binance.org |
| Polygon | Mainnet | 137 | https://polygon-rpc.com |

---

## 🚀 部署命令

### 本地网络
```bash
./scripts/deploy.sh local all
```

### 测试网
```bash
export TESTNET_DEPLOYER_PRIVATE_KEY="0x..."
./scripts/deploy.sh testnet all
```

### 主网
```bash
export MAINNET_DEPLOYER_PRIVATE_KEY="0x..."
./scripts/deploy.sh mainnet all
```

---

## 🔧 Guardian 配置

Guardian 配置文件位于 `guardian/configs/`:

- `local.toml` - 本地开发
- `testnet.toml` - 测试网
- `mainnet.toml` - 主网

### 使用示例

```bash
cd /workspace/guardian

# 本地
cargo run -- --config configs/local.toml

# 测试网
export GUARDIAN_PASSWORD="..."
cargo run -- --config configs/testnet.toml
```

---

## 🔑 密钥管理

### 本地开发
使用 Anvil 默认密钥（**仅测试用**）:
```
0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

### 测试网
```bash
# 生成新密钥
cast wallet new

# 获取测试币
# Sepolia: https://sepoliafaucet.com/
# Devnet: https://faucet.solana.com/
```

### 主网
⚠️ **使用硬件钱包或 HSM**

---

## 📚 配置文件

网络配置: `config/networks.toml`  
Guardian 配置: `guardian/configs/*.toml`  

详见配置文件内的注释说明。

---

**相关文档**:
- [08-development-plan.md](./08-development-plan.md) - 开发计划
- [10-quickstart-guide.md](./10-quickstart-guide.md) - 快速开始

