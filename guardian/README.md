# Guardian Node

Guardian节点实现 - 多签跨链桥的验证者节点

## 功能特性

- ✅ 监听 EVM 链事件 (ethers-rs)
- ✅ 监听 Solana 链事件 (WebSocket)
- 🚧 ECDSA 签名
- 🚧 P2P 签名聚合 (libp2p)
- 🚧 REST API 服务
- 🚧 VAA 生成

## 快速开始

### 编译

```bash
cargo build --release
```

### 运行

```bash
# 复制配置文件
cp config.example.toml config.toml

# 编辑配置
vim config.toml

# 运行节点
cargo run --release -- --config config.toml --log-level info
```

### 测试

```bash
cargo test
```

## 配置

参见 `config.example.toml`

## 架构

```
guardian/
├── src/
│   ├── main.rs          # 入口
│   ├── config.rs        # 配置管理
│   ├── guardian.rs      # 核心逻辑
│   ├── types.rs         # 数据类型
│   └── watcher/         # 事件监听
│       ├── mod.rs
│       ├── evm.rs       # EVM 监听器
│       └── solana.rs    # Solana 监听器
├── Cargo.toml
└── config.example.toml
```

## 下一步

- [ ] 实现完整的事件监听
- [ ] 添加签名逻辑
- [ ] 实现 P2P 网络
- [ ] 添加 REST API
- [ ] 实现 VAA 聚合

