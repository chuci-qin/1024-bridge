# 多签跨链桥项目

## 项目概述

本项目是一个基于多签验证的跨链桥系统，支持在多条 EVM 链和 SVM 链（1024chain）之间进行稳定币的跨链转移。系统采用质押-解锁机制，通过多个独立的 relayer 进行多签验证，确保跨链转账的安全性。

**支持的桥接对：**
- **Arbitrum Sepolia** USDC (6 dec) ↔ 1024chain USDC (6 dec)
- **BSC Testnet** USDT (18 dec) ↔ 1024chain USDC (6 dec) — 需配置 decimalRatio
- **ETH Sepolia** USDC (6 dec) ↔ 1024chain USDC (6 dec)

**扩展功能：** 支持从任意链到 1024chain 的跨链转账（通过成熟的跨链桥如 LiFi 实现第一步，本仓库的 cross-chain-gateway 服务完成第二步）。

## 开发状态

**当前阶段：** M4 - Relayer 服务开发（**S2E Relayer 完整实现并验证成功** ✅）

**详细进度：** 参见 [docs/progress.md](docs/progress.md)  
**测试计划：** 参见 [docs/testplan.md](docs/testplan.md)  
**API文档：** 参见 [docs/api.md](docs/api.md)  
**设计文档：** 参见 [docs/design.md](docs/design.md)

### 核心特性

- 支持多条 EVM 链（Arbitrum Sepolia / BSC Testnet / ETH Sepolia）与 SVM（1024chain）之间的双向跨链转移
- 支持不同精度代币的跨链转移（通过 decimalRatio 配置自动换算）
- 采用质押-解锁机制，而非铸币-销毁模式
- 多签验证机制，需要超过 2/3 的 relayer 签名才能完成解锁（最多支持18个relayer）
- **原生密码学算法**：SVM 使用 Ed25519，EVM 使用 ECDSA (secp256k1) + EIP-191
- 防重放攻击机制：nonce 递增判断（64位无符号整数，溢出重置为0）+ block_height
- 支持至少100个未完成的跨链请求同时存在
- 支持至少1200个签名缓存（100个请求 × 18个relayer = 1800个签名）

### 密码学算法设计

系统采用**各自原生密码学算法**的设计原则：

**SVM 端（Solana/1024chain）**：
- 签名算法：**Ed25519**（Solana 原生）
- 数据序列化：**Borsh**（Anchor 标准）
- 验证方式：**Ed25519Program** 预编译合约
- 特点：64 字节签名，性能极优

**EVM 端（Ethereum/Arbitrum）**：
- 签名算法：**ECDSA (secp256k1)**（Ethereum 原生）
- 数据序列化：**JSON 字符串**
- 哈希算法：**SHA-256 + Keccak256 (EIP-191)**
- 验证方式：**ecrecover** 预编译合约
- 特点：65 字节签名，与 Ethereum 生态完全兼容

**跨链兼容性**：
- Relayer 负责在两种格式之间转换
- 监听 SVM 事件 → 用 ECDSA 签名 → 提交到 EVM
- 监听 EVM 事件 → 用 Ed25519 签名 → 提交到 SVM

## 快速开始

### 前置条件

**EVM 工具链：**
```bash
# 安装 Foundry
curl -L https://foundry.paradigm.xyz | bash && foundryup
```

**SVM 工具链：**
```bash
# 安装 Anchor 和 Solana CLI
cargo install --git https://github.com/coral-xyz/anchor avm --locked --force
sh -c "$(curl -sSfL https://release.solana.com/stable/install)"
solana-keygen new -o ~/.config/solana/id.json
```

### 快速部署流程（单relayer模式）

```bash
cp .env.evm.deploy.example .env.evm.deploy
cp .env.svm.deploy.example .env.svm.deploy
cp .env.invoke.example .env.invoke
cp .env.config-usdc-peer.example .env.config-usdc-peer
# 填写缺失的配置
vim .env.evm.deploy  
vim .env.svm.deploy  
vim .env.invoke  
vim .env.config-usdc-peer  

cd scripts

# 1. 部署 EVM 合约
./01-deploy-evm.sh

# 2. 部署 SVM 合约（选择升级或全新部署）
./02-deploy-svm.sh

# 检查并确保 PEER_CONTRACT_ADDRESS_FOR_EVM 和 PEER_CONTRACT_ADDRESS_FOR_SVM 配置正确
vim ../.env.invoke  

# 3. 配置 USDC 和对端地址
cd -
cd scripts
./03-config-usdc-peer.sh

# 4.1 注册 Relayer（自动生成密钥）
./04-register-relayer.sh
# 之后假设 relayer 账户拥有充足的SOL和ETH支付交易费，因此需要手动向这些账户充值

# 4.2 充值 Relayer 账户（可选，用于支付 gas 费用）
./05-fund-relayer.sh

# 4.3 启动 Relayer 服务
./06-start-relayer.sh start

# 5 添加流动性：管理员从自己的账户向金库地址转入USDC
npx ts-node evm-admin.ts add_liquidity 100000000
npx ts-node svm-admin.ts add_liquidity 100000000

# 6. 测试跨链转账
npx ts-node svm-user.ts balance
npx ts-node evm-user.ts stake 100 <SVM_RECEIVER_PUBKEY>
npx ts-node svm-user.ts balance # 确认SVM余额增加

npx ts-node evm-user.ts balance
npx ts-node svm-user.ts stake 100 <EVM_RECEIVER_ADDRESS>
npx ts-node evm-user.ts balance # 确认EVM余额增加
```

详细说明见 [scripts/README.md](scripts/README.md)

### 使用 Docker 部署 Relayer

主要不同的是上面的第四步

```bash
# 确保完成上面的1~3步

cd scripts
# 4.1 生成新的relayer密钥
./04-register-relayer.sh    # 选择y覆盖现有密钥

cd ../relayer
# 4.2 初始化relayer配置文件和日志文件夹
./init-new-relayer.sh 1   # 将env文件统一复制到一个文件夹，并修改submitter的QUEUE__PATH

# 4.3 启动relayer容器
./start-container.sh 1    

# 4.4 检查relayer容器是否启动成功
docker ps | grep relayer-container-relayer1

# 4.5 运行下一个relayer
cd ../scripts
./04-register-relayer.sh    # 选择y覆盖现有密钥
cd ../relayer
./init-new-relayer.sh 2   
./start-container.sh 2    
docker ps | grep relayer-container-relayer2
```

### 多链部署（BSC Testnet / ETH Sepolia）

系统支持在多条 EVM 链上部署独立的桥接对，每条桥使用独立的 EVM 合约和 SVM Program。

#### 多桥管理工具

```bash
# 保存当前活动配置到后缀文件
cd scripts
./save-bridge.sh arb-usdc    # 保存为 .env.*.arb-usdc

# 切换到另一个桥配置
./switch-bridge.sh bnb-usdt  # 从 .env.*.bnb-usdt 加载
./switch-bridge.sh eth-usdc  # 从 .env.*.eth-usdc 加载
```

#### 各链配置参数

| 参数 | ARB-USDC | BNB-USDT | ETH-USDC |
|------|----------|----------|----------|
| EVM RPC | `https://sepolia-rollup.arbitrum.io/rpc` | `https://bsc-testnet-rpc.publicnode.com` | `https://ethereum-sepolia-rpc.publicnode.com` |
| Chain ID | 421614 | 97 | 11155111 |
| 稳定币地址 | `0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d` | `0x66E972502A34A625828C544a1914E8D8cc2A9dE5` | `0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238` |
| Decimals | 6 | 18 | 6 |
| Decimal Ratio | 1 | 1000000000000 (10^12) | 1 |
| 稳定币来源 | Circle 官方 | BSC 社区版 | Circle 官方 |
| Gas 代币 | SepoliaETH | tBNB | SepoliaETH |

#### 部署新桥的完整流程

```bash
# 1. 创建新桥的 env 文件（以 BNB-USDT 为例）
#    需要创建: .env.evm.deploy.bnb-usdt, .env.config-usdc-peer.bnb-usdt, .env.invoke.bnb-usdt

# 2. 切换到新桥配置
cd scripts
./switch-bridge.sh bnb-usdt

# 3. 部署 EVM 合约
./01-deploy-evm.sh

# 4. 部署新的 SVM Program（生成新 Program ID）
./02-deploy-svm.sh    # 选择全新部署

# 5. 配置（含 decimalRatio）
./03-config-usdc-peer.sh

# 6. 注册并启动 3 个 Docker Relayer（编号 4,5,6）
./04-register-relayer.sh    # 选y
cd ../relayer
./init-new-relayer.sh 4 && ./start-container.sh 4
cd ../scripts && ./04-register-relayer.sh
cd ../relayer && ./init-new-relayer.sh 5 && ./start-container.sh 5
cd ../scripts && ./04-register-relayer.sh
cd ../relayer && ./init-new-relayer.sh 6 && ./start-container.sh 6

# 7. 保存配置
cd ../scripts
./save-bridge.sh bnb-usdt
```

#### Decimal Ratio 机制

当 EVM 链上的稳定币精度（decimals）与 1024chain USDC 不同时，需配置 `decimalRatio`：

- **EVM→SVM 方向**：`stake()` 自动将金额除以 ratio 后发送事件（转为 6-decimal）
- **SVM→EVM 方向**：`unlock()` 自动将金额乘以 ratio 后解锁（还原为源链精度）
- `ratio = 10^(源链精度 - 6)`，例如 USDT(18dec) 设为 `10^12`，USDC(6dec) 保持默认 `1`

在 `.env.config-usdc-peer` 中设置 `DECIMAL_RATIO`，`03-config-usdc-peer.sh` 会自动调用合约配置。

#### Relayer 编号规范

| 桥 | Relayer 编号 | 端口范围 |
|----|------------|---------|
| ARB-USDC | 1, 2, 3 | 8081-8083, 8181-8183, 8281-8283 |
| BNB-USDT | 4, 5, 6 | 8381-8383, 8481-8483, 8581-8583 |
| ETH-USDC | 7, 8, 9 | 8681-8683, 8781-8783, 8881-8883 |

#### 测试网代币获取

| 链 | Gas 代币 | 获取方式 | 稳定币 | 获取方式 |
|----|---------|---------|--------|---------|
| ARB Sepolia | SepoliaETH | [Alchemy Faucet](https://www.alchemy.com/faucets/arbitrum-sepolia) | USDC | [Circle Faucet](https://faucet.circle.com/) |
| BSC Testnet | tBNB | [BNB Faucet](https://www.bnbchain.org/en/testnet-faucet) | USDT | `cast send` 调用合约 mint() |
| ETH Sepolia | SepoliaETH | [Google Cloud Faucet](https://cloud.google.com/application/web3/faucet/ethereum/sepolia) | USDC | [Circle Faucet](https://faucet.circle.com/) |
| 1024chain | SOL | `solana airdrop 2 <addr> --url https://rpc-testnet.1024chain.com/rpc/` | USDC | 管理员 mint |

管理员钱包地址: `0xd4b42eff8af8ef82de3830fe30559bff92dca55f`（所有 EVM 链共用同一私钥）

## 使用方法

### 跨链转账流程

1. 用户在发送端链调用质押接口，传入质押数量和接收端地址
2. 发送端合约将用户的 USDC 转入质押金库，并触发质押事件
3. 多个 relayer 监听到质押事件后，分别对事件数据进行签名
4. 每个 relayer 将签名后的数据发送到接收端合约
5. 接收端合约验证签名，当达到 2/3 阈值后，从金库解锁等量 USDC 到接收地址

## 项目结构

### 智能合约

- **svm/**：Solana 智能合约（1024chain 部署）
  - **统一初始化**：`initialize` 函数同时初始化发送端和接收端合约
  - **USDC配置**：`configure_usdc` 函数配置USDC mint account地址
  - **统一对端配置**：`configure_peer` 函数同时配置发送端和接收端的对端信息
  - 发送端合约：负责质押 USDC 并触发质押事件（nonce自动递增）
  - 接收端合约：验证 relayer **Ed25519** 签名并解锁 USDC（使用nonce递增判断防重放）
  - 每个跨链请求使用独立的 CrossChainRequest PDA 账户存储签名缓存
  - **密码学**：Ed25519 签名 + Borsh 序列化（Solana 原生）
  
- **evm/**：EVM 智能合约（Arbitrum Sepolia 部署）
  - **初始化**：`initialize` 函数初始化发送端和接收端合约
  - **USDC配置**：`configure_usdc` 函数配置USDC ERC20合约地址
  - **对端配置**：`configure_peer` 函数配置对端合约和链ID
  - 发送端合约：负责质押 USDC 并触发质押事件
  - 接收端合约：验证 relayer **ECDSA** 签名并解锁 USDC
  - **金库设计（v2.0）**：合约本身作为金库，无需外部 vault 或 approve
  - **密码学**：ECDSA (secp256k1) + EIP-191 + SHA-256 + JSON（Ethereum 原生）

### 中继服务

- **relayer/**：中继服务器（Rust 实现 🦀）
  - **s2e 服务** (SVM→EVM)：✅ **完整实现并验证**
    - 单一进程架构
    - 监听 1024chain 事件（HTTP RPC 轮询）
    - ECDSA 签名（SHA-256 + EIP-191）
    - 提交到 Arbitrum
    - HTTP API（端口 8081）
    - 详细说明：[relayer/README_S2E.md](relayer/README_S2E.md)
  - **e2s 服务** (EVM→SVM)：✅ **完整实现并运行**
    - 分离式架构（解决依赖冲突）
    - `e2s-listener`：监听 Arbitrum 事件 → 文件队列
    - `e2s-submitter`：队列处理 → Ed25519 签名 → 提交到 1024chain
    - HTTP API（端口 8082）
    - 详细说明：[relayer/README_E2S.md](relayer/README_E2S.md)
  - **HTTP API**：健康检查、状态查询、Prometheus 指标
  - **高性能架构**：Tokio 异步运行时 + 文件队列（e2s）/ 内存队列（s2e）
  - 详细设计见 [relayer/README.md](relayer/README.md)

### 跨链网关服务（Broker）

- **broker/**：跨链网关服务，实现两段式跨链桥方案 ✅ **已完整实现**
  
  #### 两段式跨链桥架构
  
  系统采用**两段式跨链桥**设计，通过成熟的跨链桥（LiFi SDK）和自定义 Broker 服务，实现任意链与 1024chain 之间的跨链转账：
  
  **Deposit 方向（存入）：任意链 → Arbitrum → 1024chain** ✅
  1. **第一步**：用户使用 LiFi SDK 将资产从任意链跨链到 Arbitrum 的 USDC
     - 支持的源链：Ethereum、Polygon、BSC、Avalanche、Base、Optimism、Arbitrum 等
     - 支持的源代币：各链上的原生代币或稳定币
     - 目标：Arbitrum 上的 USDC
     - USDC 转入 Broker 的中转钱包地址
  2. **第二步**：调用 Broker EVM Gateway Service 完成从 Arbitrum 到 1024chain 的跨链
     - HTTP API：`POST /stake`（端口 8084）
     - 参数：`amount`（USDC 金额）、`target_address`（1024chain 接收地址）
     - 服务使用中转钱包自动调用 EVM stake 合约接口
     - 自动检查 USDC 余额和授权
  
  **Withdraw 方向（提取）：1024chain → Arbitrum → 任意链** ✅
  1. **第一步**：用户在 1024chain 调用 SVM stake 合约，将 USDC 发送到 Broker 的 Arbitrum 地址
     - 用户调用 SVM 合约的 `stake` 方法
     - 参数：`amount`（USDC 金额）、`receiver_address`（Broker 的 Arbitrum 地址）
     - USDC 从 1024chain 跨链到 Arbitrum，转入 Broker 的中转钱包
  2. **第二步**：调用 Broker Withdraw Gateway Service 完成从 Arbitrum 到目标链的跨链
     - HTTP API：`POST /withdraw`（端口 8085）
     - 参数：`target_chain`（目标链 ID）、`target_asset`（目标代币地址）、`usdc_amount`（USDC 金额）、`recipient_address`（接收地址）
     - 服务使用 LiFi SDK 自动执行跨链交易
     - 支持跨链到任意链的任意代币
  
  #### Broker 服务组件
  
  - **evm-gateway-service**：EVM 网关服务（Rust 实现）✅
    - 负责 Deposit 方向的第二步：Arbitrum → 1024chain
    - HTTP API（端口 8084）
    - 详细说明：[broker/evm-gateway-service/README.md](broker/evm-gateway-service/README.md)
  
  - **withdraw-gateway-service**：提现网关服务（TypeScript/Node.js 实现）✅
    - 负责 Withdraw 方向的第二步：Arbitrum → 任意链
    - HTTP API（端口 8085）
    - 集成 LiFi SDK 实现跨链
    - 支持速率限制和并发控制
    - 详细说明：[broker/withdraw-gateway-service/README.md](broker/withdraw-gateway-service/README.md)
  
  #### 架构说明
  
  Broker 服务与 `relayer` 完全独立，职责不同：
  - **relayer**：负责监听链上事件、签名验证、多签提交（双向跨链：EVM ↔ SVM）
  - **broker**：负责接收外部 HTTP 请求，完成两段式跨链桥的第二步（单向：Arbitrum → 1024chain 或 1024chain → 任意链）
  
  #### 工作流程示例
  
  **Deposit 流程：**
  ```
  用户钱包（Ethereum USDC）
    ↓ [LiFi SDK 跨链]
  Broker 中转钱包（Arbitrum USDC）
    ↓ [Broker EVM Gateway Service]
  EVM Stake 合约（Arbitrum）
    ↓ [Relayer 多签验证]
  1024chain 用户地址（USDC）
  ```
  
  **Withdraw 流程：**
  ```
  1024chain 用户地址（USDC）
    ↓ [SVM Stake 合约]
  Broker 中转钱包（Arbitrum USDC）
    ↓ [Broker Withdraw Gateway Service + LiFi SDK]
  用户钱包（目标链目标代币）
  ```

### 部署和运维脚本

- **scripts/**：部署和操作脚本（TypeScript + Shell）- **已精简至 11 个核心脚本**
  - **部署脚本**：
    - `01-deploy-evm.sh`：自动化部署 EVM 合约到 EVM 链（支持 Arbitrum/BSC/ETH 等）
    - `02-deploy-svm.sh`：自动化部署 SVM 合约到 1024chain（支持升级/全新部署）
    - `03-config-usdc-peer.sh`：配置 USDC 地址和对端合约地址
    - `04-register-relayer.sh`：自动生成并注册 Relayer 密钥对
    - `05-fund-relayer.sh`：为 Relayer 账户充值（ETH 和 SOL）
    - `06-start-relayer.sh`：启动/停止 Relayer 服务（s2e, e2s-listener, e2s-submitter）
  - **管理脚本**：
    - `evm-admin.ts`：EVM 合约管理操作（查询状态、配置、relayer 管理、流动性管理）
    - `svm-admin.ts`：SVM 合约管理操作（查询状态、配置、relayer 管理、流动性管理）
  - **用户脚本**：
    - `evm-user.ts`：EVM 用户操作（质押 USDC、查询余额）
    - `svm-user.ts`：SVM 用户操作（质押 USDC、查询余额）
  - **多桥管理脚本**：
    - `save-bridge.sh <profile>`：保存当前活动配置到后缀文件
    - `switch-bridge.sh <profile>`：切换到指定桥配置
  - **测试脚本**：
    - `cross-chain-test.ts`：EVM→SVM 端到端测试
    - `cross-chain-test-s2e.ts`：SVM→EVM 端到端测试
  - 详细文档见 [scripts/README.md](scripts/README.md)

### 文档

- **README.md**：项目概述和使用说明（本文件）
- **docs/api.md**：API 接口文档和模块间调用规约
- **docs/testplan.md**：测试计划和用户故事
- **docs/progress.md**：项目进度跟踪

## 配置说明

系统需要配置以下参数以支持不同网络环境（测试网/主网）：

- 发送端链的 RPC 地址
- 接收端链的 RPC 地址
- 发送端合约地址
- 接收端合约地址
- 质押金库地址：
  - **SVM端**：PDA 金库地址（发送端和接收端共享同一个金库）
  - **EVM端（v2.0）**：合约本身作为金库，不需要单独配置
- 管理员钱包地址（SVM 和 EVM 各自独立，但在SVM中发送端和接收端共享同一个管理员）
- 稳定币代币地址：
  - SVM端：USDC mint account地址（通过 `configure_usdc` 配置）
  - EVM端：稳定币 ERC20 合约地址（通过 `configure_usdc` 配置，支持 USDC/USDT 等任意 ERC20）
- **Decimal Ratio**（可选）：当 EVM 代币精度与 SVM USDC 不同时配置（通过 `configure_decimal_ratio` 配置）
- Relayer 私钥列表（最多18个relayer）
- Chain ID（支持 Arbitrum Sepolia 421614 / BSC Testnet 97 / ETH Sepolia 11155111 / 1024chain 91024）

### EVM v2.0 金库变更

- ✅ **合约即金库**：合约地址直接持有稳定币，不需要外部 vault
- ✅ **简化部署**：不需要配置 vault 地址或进行 approve 操作
- ✅ **流动性管理**：直接向合约地址转入稳定币即可增加流动性

### 多链 Decimal Ratio 支持

- ✅ **自动精度换算**：EVM 合约内置 `decimalRatio` 配置，支持不同精度代币自动换算
- ✅ **向后兼容**：默认 `ratio=1`，已有的同精度桥（如 ARB-USDC）无需额外配置
- ✅ **SVM/Relayer 无需修改**：所有换算在 EVM 合约层完成
- ✅ **多桥管理工具**：`switch-bridge.sh` / `save-bridge.sh` 支持快速切换多桥配置
