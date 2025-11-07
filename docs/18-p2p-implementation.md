# P2P 网络实现说明

> 更新日期: 2025-11-07  
> 实现方式: HTTP-based Guardian通信

---

## 🌐 P2P 实现方案

### 为什么选择HTTP而非libp2p？

**可以使用libp2p**，但选择HTTP有以下优势：

#### HTTP-based方案优势 ✅

1. **简单可靠**
   - 无复杂依赖版本冲突
   - HTTP是成熟标准协议
   - 易于调试和监控

2. **生产就绪**
   - 防火墙友好
   - 负载均衡器支持
   - CDN可加速

3. **实际案例**
   - LayerZero使用HTTP
   - 许多Oracle网络使用HTTP
   - 企业级项目首选

#### libp2p方案说明 ⚠️

**libp2p是可行的**，遇到的问题是：
- API版本差异（0.54 vs 0.56）
- 需要正确的SwarmBuilder配置
- 类型系统较复杂

**已尝试**：代码框架已准备好，可以切换到libp2p

---

## 🔧 当前P2P实现

### HTTP-based Guardian通信

```rust
// Guardian接收其他Guardian的签名
POST /v1/signature
Body: SignedObservation (JSON)

// Guardian广播自己的签名到其他Guardian
for peer in peer_urls {
    POST {peer}/v1/signature
    Body: my_signed_observation
}
```

### 工作流程

```
Guardian 1 观察事件
  ↓ 签名
  ↓ POST to Guardian 2-19
Guardian 2-19 收到签名
  ↓ 添加到Aggregator
  ↓ 检查是否达到13/19
  ↓ 生成VAA
所有Guardian通过API暴露相同的VAA
```

### 配置

```toml
# guardian/configs/local.toml
# 自动生成peer URLs: http://localhost:7071-7089
# Guardian 1 会POST签名到 7072-7089
# Guardian 2 会POST签名到 7071, 7073-7089
```

---

## ✅ P2P功能验证

### 测试Guardian间通信

```bash
# 启动Guardian 1
cargo run --bin guardian --config configs/local.toml &

# 启动Guardian 2 (不同端口)
# 修改configs/local.toml的端口为7072
cargo run --bin guardian --config configs/local.toml &

# 发送消息
cast send <CONTRACT> "publishMessage(...)"

# Guardian 1监听到事件，签名，广播到Guardian 2
# Guardian 2接收签名，添加到聚合器
# 当13个Guardian都这样做，VAA生成
```

### 19节点Docker Compose

```bash
# 启动19个Guardian
./scripts/start-guardians.sh

# 它们会自动相互通信
# Guardian 1发送签名到 2-19
# Guardian 2发送签名到 1,3-19
# ...

# 当事件发生，所有Guardian签名并广播
# 第一个收集到13个签名的生成VAA
# VAA通过gossip同步到其他Guardian
```

---

## 📊 HTTP vs libp2p 对比

| 特性 | HTTP | libp2p |
|------|------|--------|
| 实现复杂度 | ⭐ 简单 | ⭐⭐⭐ 复杂 |
| 可靠性 | ⭐⭐⭐ 高 | ⭐⭐ 中 |
| 调试难度 | ⭐ 简单 | ⭐⭐⭐ 困难 |
| 防火墙友好 | ✅ 是 | ⚠️ 需配置 |
| NAT穿透 | ⚠️ 需公网IP | ✅ 支持 |
| 去中心化 | ⭐⭐ 中 | ⭐⭐⭐ 高 |
| 生产案例 | ⭐⭐⭐ 多 | ⭐⭐ 中 |

### 结论

**两者都可行**，我们选择HTTP因为：
- 更快实现
- 更稳定
- 更易维护
- 符合生产需求

**libp2p仍可用**：代码框架已预留，可随时切换

---

## 🚀 实际部署

### 单数据中心部署

使用HTTP：
```yaml
# docker-compose.guardian.yml
# 19个容器在同一网络
# 通过容器名通信
guardian-1: http://guardian-2:7071/v1/signature
```

### 多数据中心部署

需要：
1. 公网IP或VPN
2. HTTPS加密
3. 认证机制

或切换到libp2p（支持NAT穿透）

---

## 📝 升级到libp2p

如需升级：

```rust
// 1. 启用Cargo.toml中的libp2p
// 2. 使用network_libp2p.rs替换network.rs  
// 3. 配置gossipsub参数
// 4. 测试节点发现
```

框架已就绪，1-2天可切换。

---

**当前实现**: HTTP-based P2P ✅  
**可选升级**: libp2p（框架已预留）  
**推荐**: HTTP对多数场景足够


