# 开发环境快速参考

> 常用命令和配置速查表

---

## 🐳 Docker 环境

### 管理命令

```bash
# 构建环境
./scripts/dev.sh build

# 启动环境 (后台)
./scripts/dev.sh start

# 进入开发容器
./scripts/dev.sh shell

# 查看状态
./scripts/dev.sh status

# 查看日志
./scripts/dev.sh logs

# 重启环境
./scripts/dev.sh restart

# 停止环境
./scripts/dev.sh stop

# 清理环境 (⚠️ 删除所有数据)
./scripts/dev.sh clean
```

### 端口映射

| 服务 | 容器端口 | 主机端口 | 用途 |
|------|---------|---------|------|
| Anvil (EVM) | 8545 | 8545 | HTTP RPC |
| Anvil (EVM) | 8546 | 8546 | WebSocket |
| Solana RPC | 8899 | 8899 | HTTP RPC |
| Solana WebSocket | 8900 | 8900 | WebSocket |
| Solana Faucet | 9900 | 9900 | 空投服务 |
| Guardian-1 | 7071 | 7071 | API |
| Guardian-1 P2P | 4001 | 4001 | libp2p |

---

## ⚙️ 本地测试网

### 启动 EVM 测试网 (Anvil)

```bash
# 基础启动
anvil --host 0.0.0.0 --port 8545

# 带参数启动
anvil \
  --host 0.0.0.0 \
  --port 8545 \
  --chain-id 1337 \
  --accounts 10 \
  --balance 10000 \
  --gas-limit 30000000
```

**默认账户**:
- 私钥: `0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80`
- 地址: `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266`
- 余额: 10,000 ETH

### 启动 Solana 测试网

```bash
# 基础启动
solana-test-validator

# 带参数启动 (推荐)
solana-test-validator \
  --rpc-port 8899 \
  --faucet-port 9900 \
  --ledger /tmp/test-ledger \
  --reset

# 空投 SOL
solana airdrop 100 <address> --url http://localhost:8899
```

**配置 Solana CLI**:
```bash
# 设置集群
solana config set --url http://localhost:8899

# 生成密钥对
solana-keygen new --outfile ~/.config/solana/id.json

# 查看地址
solana address

# 查看余额
solana balance
```

---

## 🔨 合约开发

### EVM (Foundry)

```bash
cd /workspace/contracts/evm

# 初始化项目
forge init

# 安装依赖
forge install OpenZeppelin/openzeppelin-contracts

# 编译
forge build

# 测试 (详细输出)
forge test -vvv

# 测试特定合约
forge test --match-contract CoreContractTest

# 格式化代码
forge fmt

# 生成 gas 报告
forge test --gas-report

# 部署
forge create src/CoreContract.sol:CoreContract \
  --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

### Solana (Anchor)

```bash
cd /workspace/programs/solana-core

# 初始化项目
anchor init solana-core

# 构建
anchor build

# 测试
anchor test

# 部署到本地网络
anchor deploy --provider.cluster localnet

# 部署到开发网
anchor deploy --provider.cluster devnet

# 查看程序 ID
anchor keys list

# 验证程序
solana program show <PROGRAM_ID> --url http://localhost:8899
```

**Anchor.toml 配置**:
```toml
[provider]
cluster = "localnet"
wallet = "~/.config/solana/id.json"

[programs.localnet]
solana_core = "Core11111111111111111111111111111111111111"

[scripts]
test = "yarn run ts-mocha -p ./tsconfig.json -t 1000000 tests/**/*.ts"
```

---

## 🔍 Guardian 节点

### 编译与运行

```bash
cd /workspace/guardian

# 编译
cargo build --release

# 运行
cargo run --release -- \
  --config configs/guardian-1.yaml \
  --log-level info

# 后台运行
nohup cargo run --release -- \
  --config configs/guardian-1.yaml > guardian.log 2>&1 &

# 测试
cargo test

# 代码覆盖率
cargo tarpaulin --out Html
```

### 配置文件示例

```yaml
# guardian-1.yaml
guardian:
  index: 1
  
  keystore:
    path: "/workspace/guardian/keys/guardian-1.key"
    password_env: "GUARDIAN_PASSWORD"
  
  p2p:
    listen_addr: "/ip4/0.0.0.0/tcp/4001"
    bootstrap_peers:
      - "/ip4/127.0.0.1/tcp/4002"
  
  chains:
    evm:
      rpc_url: "ws://localhost:8545"
      core_contract: "0x..."
    solana:
      rpc_url: "http://localhost:8899"
      core_program: "Core1111..."
  
  api:
    listen: "0.0.0.0:7071"
```

### API 调用

```bash
# 健康检查
curl http://localhost:7071/v1/health

# 获取 VAA
curl http://localhost:7071/v1/signed_vaa/1/0x1234.../42

# 查看 Guardian 状态
curl http://localhost:7071/v1/guardian/status
```

---

## 🛠️ 中继工具

### CLI 使用

```bash
cd /workspace/relayer/cli

# 构建
cargo build --release

# 获取 VAA
./target/release/bridge-cli fetch-vaa \
  --guardian-url http://localhost:7071 \
  --chain 1 \
  --emitter 0x1234... \
  --sequence 42 \
  --output vaa.bin

# 提交 VAA 到 Solana
./target/release/bridge-cli submit-vaa \
  --chain solana \
  --rpc-url http://localhost:8899 \
  --vaa-file vaa.bin \
  --payer ~/.config/solana/id.json

# 提交 VAA 到 EVM
./target/release/bridge-cli submit-vaa \
  --chain evm \
  --rpc-url http://localhost:8545 \
  --vaa-file vaa.bin \
  --private-key 0xac09...
```

---

## 🧪 测试

### 单元测试

```bash
# EVM 合约
cd contracts/evm
forge test -vvv

# Solana 程序
cd programs/solana-core
anchor test

# Guardian 节点
cd guardian
cargo test
```

### 集成测试

```bash
cd tests

# 安装依赖
npm install

# 运行所有测试
npm run test:all

# EVM → Solana
npm run test:evm-to-solana

# Solana → EVM
npm run test:solana-to-evm

# 性能测试
npm run test:performance
```

---

## 📝 常用 Rust 命令

```bash
# 格式化代码
cargo fmt

# 检查代码 (不编译)
cargo check

# Linter
cargo clippy

# 更新依赖
cargo update

# 清理构建产物
cargo clean

# 查看依赖树
cargo tree

# 生成文档
cargo doc --open
```

---

## 🐛 调试技巧

### EVM 合约调试

```bash
# 使用 Foundry 调试器
forge test --debug <test_name>

# 查看交易 trace
cast run <tx_hash> --rpc-url http://localhost:8545

# 查看合约存储
cast storage <contract_address> <slot> --rpc-url http://localhost:8545
```

### Solana 程序调试

```bash
# 查看程序日志
solana logs -u http://localhost:8899

# 查看账户数据
solana account <address> --url http://localhost:8899

# 查看交易详情
solana confirm <signature> --url http://localhost:8899
```

### Guardian 日志

```bash
# 实时查看日志
tail -f /workspace/guardian/guardian.log

# 筛选错误
grep "ERROR" /workspace/guardian/guardian.log

# JSON 格式化
cat guardian.log | jq .
```

---

## 🔑 密钥管理

### EVM 密钥

```bash
# 使用 cast 生成密钥
cast wallet new

# 导出私钥 (⚠️ 危险)
cast wallet address --private-key 0x...

# 签名消息
cast wallet sign "Hello World" --private-key 0x...
```

### Solana 密钥

```bash
# 生成新密钥
solana-keygen new --outfile key.json

# 查看公钥
solana-keygen pubkey key.json

# 导出私钥 (base58)
solana-keygen pubkey key.json --outfile /dev/null

# 恢复密钥
solana-keygen recover
```

### Guardian 密钥

```bash
cd /workspace/guardian

# 生成 Guardian 密钥
cargo run --bin keygen -- \
  --output keys/guardian-1.key \
  --password-env GUARDIAN_PASSWORD

# 导出公钥
cargo run --bin keygen -- \
  --keyfile keys/guardian-1.key \
  --show-pubkey
```

---

## 📊 监控指标

### 系统资源

```bash
# 容器资源使用
docker stats multisig-bridge-dev

# 磁盘使用
df -h

# 内存使用
free -h

# CPU 使用
top
```

### 链状态

```bash
# EVM 区块高度
cast block-number --rpc-url http://localhost:8545

# Solana 区块高度
solana block-height --url http://localhost:8899

# EVM gas price
cast gas-price --rpc-url http://localhost:8545

# Solana 性能
solana performance-stats --url http://localhost:8899
```

---

## 🆘 故障排查

### 常见问题

| 问题 | 解决方案 |
|------|---------|
| 容器无法启动 | `docker-compose down -v && docker-compose up -d` |
| 端口被占用 | `lsof -i :8545` 查找并 kill 进程 |
| Anvil 无法连接 | 检查 `--host 0.0.0.0` 参数 |
| Solana 空投失败 | 确保 faucet 运行: `solana-test-validator --faucet-port 9900` |
| Guardian 签名失败 | 检查密钥路径和环境变量 |
| VAA 验证失败 | 确认 Guardian Set 索引一致 |

### 重置环境

```bash
# 重置 Docker 环境
./scripts/dev.sh clean
./scripts/dev.sh build
./scripts/dev.sh start

# 重置 Solana 账本
solana-test-validator --reset

# 重置 Anvil (自动)
# Anvil 每次重启都是全新状态
```

---

## 📚 参考链接

- [Foundry Book](https://book.getfoundry.sh/)
- [Anchor Book](https://www.anchor-lang.com/)
- [Solana Cookbook](https://solanacookbook.com/)
- [ethers-rs Docs](https://docs.rs/ethers/)
- [libp2p Docs](https://docs.libp2p.io/)

---

**最后更新**: 2025-11-06
