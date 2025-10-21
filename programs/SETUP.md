# Solana Bridge Program 设置指南

> **目标**: 在1024Chain testnet部署Bridge Program  
> **前提**: testnet上没有USDC，需要先创建

---

## 🎯 完整流程

### 第1步: 配置Solana CLI（5分钟）

```bash
# 设置RPC为你的1024Chain testnet
solana config set --url https://testnet-rpc.1024chain.com/rpc/

# 验证连接
solana cluster-version

# 检查余额
solana balance

# 如果余额为0，获取测试SOL
# 选项A: Airdrop（如果testnet支持）
solana airdrop 2

# 选项B: 从faucet获取
# 访问你的testnet faucet网站
```

---

### 第2步: 创建USDC Token（10分钟）⭐

**安装spl-token CLI**（如果没有）:
```bash
cargo install spl-token-cli
```

**创建USDC Token**:
```bash
# 1. 创建Token（6位小数，像真实USDC）
spl-token create-token --decimals 6

# 记录输出的Mint地址，例如：
# Creating token ABC123...xyz
# 
# Address: ABC123...xyz  ⭐ 这就是USDC Mint地址
```

**重要**: 保存这个Mint地址！后面会用到！

---

**设置Token元数据**（可选但推荐）:
```bash
# 设置Token名称和符号
spl-token create-account <MINT_ADDRESS>

# 设置元数据（需要Metaplex）
# Token Name: USD Coin
# Symbol: USDC
# Decimals: 6
```

---

**初始铸造**（测试用）:
```bash
# 给自己铸造一些测试USDC
spl-token mint <MINT_ADDRESS> 1000000

# 1000000 = 1,000,000 USDC（6位小数）
```

---

### 第3步: 编译Bridge Program（5分钟）

**如果有Anchor CLI**:
```bash
cd programs
anchor build
```

**如果没有Anchor CLI**（手动编译）:
```bash
cd programs/bridge

# 编译
cargo build-sbf

# 输出在：
# target/deploy/bridge.so
```

---

### 第4步: 部署Program（5分钟）

**使用Anchor**:
```bash
anchor deploy
```

**手动部署**:
```bash
solana program deploy target/deploy/bridge.so

# 记录Program ID，例如：
# Program Id: DEF456...xyz  ⭐ 保存这个！
```

---

### 第5步: 设置Bridge为USDC Mint Authority（关键！）

**为什么**: Bridge Program需要mint权限才能铸造USDC

**操作**:
```bash
# 把USDC的mint authority转给Bridge Program
spl-token authorize <USDC_MINT_ADDRESS> mint <BRIDGE_PROGRAM_ID>

# 验证
spl-token display <USDC_MINT_ADDRESS>

# 应该看到：
# Mint authority: <BRIDGE_PROGRAM_ID>  ✅
```

**关键**: 这一步完成后，只有Bridge Program能mint USDC！

---

### 第6步: 初始化Bridge State（5分钟）

**编写初始化脚本**: `migrations/initialize.ts`

```typescript
import * as anchor from "@coral-xyz/anchor";

async function main() {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  
  const program = anchor.workspace.Bridge;
  
  // 初始化Bridge State
  const tx = await program.methods.initialize().rpc();
  
  console.log("✅ Bridge initialized!");
  console.log("   Transaction:", tx);
}

main();
```

**运行**:
```bash
ts-node migrations/initialize.ts
```

---

## 📋 配置文件

### 环境变量（.env）

```bash
# 1024Chain testnet RPC
SOLANA_RPC_URL=https://testnet-rpc.1024chain.com/rpc/

# USDC Mint地址（第2步创建的）
USDC_MINT_ADDRESS=ABC123...xyz

# Bridge Program ID（第4步部署的）
BRIDGE_PROGRAM_ID=DEF456...xyz

# 钱包
WALLET_PATH=~/.config/solana/id.json
```

---

### Anchor.toml（已创建）

**关键配置**:
```toml
[provider]
cluster = "https://testnet-rpc.1024chain.com/rpc/"  ⭐
wallet = "~/.config/solana/id.json"
```

---

## 🧪 测试

### 本地测试

**编写**: `tests/bridge.ts`

```typescript
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Bridge } from "../target/types/bridge";

describe("bridge", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Bridge as Program<Bridge>;

  it("Initialize", async () => {
    const tx = await program.methods.initialize().rpc();
    console.log("Initialize tx:", tx);
  });

  it("Mint USDC", async () => {
    const amount = new anchor.BN(1000_000000);  // 1000 USDC
    const arbTxHash = "0x123...";
    
    const tx = await program.methods.mintUsdc(amount, arbTxHash).rpc();
    console.log("Mint tx:", tx);
  });

  it("Burn USDC", async () => {
    const amount = new anchor.BN(100_000000);   // 100 USDC
    const arbAddress = "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb";
    
    const tx = await program.methods.burnUsdc(amount, arbAddress).rpc();
    console.log("Burn tx:", tx);
  });
});
```

**运行**:
```bash
anchor test
```

---

## ✅ 完成检查清单

### USDC Token创建

- [ ] spl-token create-token完成
- [ ] 记录Mint地址
- [ ] 铸造测试USDC
- [ ] 验证Token存在

---

### Bridge Program部署

- [ ] 程序编译成功
- [ ] 程序部署成功
- [ ] 记录Program ID
- [ ] 设置mint authority
- [ ] 初始化Bridge State

---

### 测试验证

- [ ] 本地测试通过
- [ ] Mint测试成功
- [ ] Burn测试成功
- [ ] 重放保护测试

---

## 🎯 估计时间

| 任务 | 时间 |
|------|------|
| 配置Solana CLI | 5分钟 |
| 创建USDC Token | 10分钟 ⭐ |
| 编译Program | 5分钟 |
| 部署Program | 5分钟 |
| 设置Mint Authority | 5分钟 ⭐ |
| 初始化Bridge | 5分钟 |
| 编写测试 | 2小时 |
| 测试和调试 | 3小时 |
| **总计** | **~6小时** |

**Day 3可以完成！** ✅

---

## 📚 快速命令参考

```bash
# 配置RPC
solana config set --url https://testnet-rpc.1024chain.com/rpc/

# 创建USDC
spl-token create-token --decimals 6

# 铸造测试USDC
spl-token mint <MINT> 1000000

# 编译程序
cargo build-sbf

# 部署
solana program deploy target/deploy/bridge.so

# 设置authority
spl-token authorize <MINT> mint <PROGRAM_ID>
```

---

**程序代码已创建！** ✅

**准备第2步：创建USDC Token！** 🪙

