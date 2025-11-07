# 开发计划与进度跟踪

> 文档版本: v1.0  
> 创建日期: 2025-11-07  
> 状态: 🚧 开发中

---

## 📊 项目状态

**当前阶段**: Phase 1 - 基础设施搭建

**完成度**: 
- 设计阶段: ✅ 100%
- 开发阶段: 🚧 5%
- 测试阶段: ⏳ 0%

---

## 🎯 开发路线图

### Phase 1: 基础设施搭建 (Week 1-2) 🚧

**目标**: 搭建本地开发环境,完成基础合约

**任务列表**:
- [x] 1.0 技术调研与设计 (已完成)
- [ ] 1.1 Docker开发环境搭建
  - [ ] 创建 Dockerfile
  - [ ] 编写 docker-compose.yml
  - [ ] 配置开发工具脚本
- [ ] 1.2 EVM Core Contract 开发
  - [ ] 合约基础结构
  - [ ] Guardian Set 管理
  - [ ] 消息发送逻辑
  - [ ] VAA 验证逻辑
  - [ ] 单元测试
- [ ] 1.3 Solana Core Program 开发
  - [ ] Anchor 项目初始化
  - [ ] Bridge 账户结构
  - [ ] 消息发布指令
  - [ ] VAA 验证指令
  - [ ] 测试用例
- [ ] 1.4 本地测试网启动脚本
  - [ ] Anvil 启动脚本
  - [ ] Solana Test Validator 启动脚本
  - [ ] 合约部署脚本

**交付物**:
- ✅ Dockerfile 和 docker-compose.yml
- ✅ 可运行的 EVM Core Contract
- ✅ 可运行的 Solana Core Program
- ✅ 本地测试环境启动脚本

---

### Phase 2: Guardian 节点实现 (Week 3-4)

**目标**: 实现完整的 Guardian 节点

**任务列表**:
- [ ] 2.1 Guardian 节点框架
  - [ ] Rust 项目结构搭建
  - [ ] 配置文件解析
  - [ ] 日志系统
  - [ ] 主运行循环
- [ ] 2.2 EVM Watcher
  - [ ] WebSocket 连接管理
  - [ ] 事件监听与解析
  - [ ] 确认块等待逻辑
- [ ] 2.3 Solana Watcher
  - [ ] WebSocket logsSubscribe
  - [ ] 交易日志解析
  - [ ] Slot 追踪
- [ ] 2.4 P2P 网络
  - [ ] libp2p 集成
  - [ ] Gossipsub 配置
  - [ ] 签名广播
- [ ] 2.5 签名逻辑
  - [ ] ECDSA 签名实现
  - [ ] 密钥管理
  - [ ] 签名验证

**交付物**:
- ✅ 可运行的 Guardian 节点
- ✅ 事件监听功能
- ✅ P2P 网络通信
- ✅ 签名功能

---

### Phase 3: VAA 系统 (Week 5)

**目标**: 实现 VAA 生成与聚合

**任务列表**:
- [ ] 3.1 VAA 数据结构
  - [ ] Rust 结构定义
  - [ ] 序列化/反序列化
  - [ ] 摘要计算
- [ ] 3.2 签名聚合
  - [ ] 签名收集逻辑
  - [ ] 法定人数检查
  - [ ] VAA 组装
- [ ] 3.3 Guardian API
  - [ ] REST API 框架
  - [ ] VAA 查询端点
  - [ ] 健康检查端点

**交付物**:
- ✅ VAA 生成功能
- ✅ Guardian REST API
- ✅ 19 节点 Docker Compose 配置

---

### Phase 4: 中继工具 (Week 6)

**目标**: 实现用户手动中继工具

**任务列表**:
- [ ] 4.1 CLI 工具框架
  - [ ] Clap 参数解析
  - [ ] 命令结构
- [ ] 4.2 VAA 获取
  - [ ] Guardian API 客户端
  - [ ] 重试逻辑
- [ ] 4.3 VAA 提交
  - [ ] EVM 合约交互
  - [ ] Solana 程序交互
  - [ ] Gas 管理

**交付物**:
- ✅ CLI 中继工具
- ✅ 使用文档

---

### Phase 5: 集成测试 (Week 7)

**目标**: 端到端测试验证

**任务列表**:
- [ ] 5.1 测试场景设计
- [ ] 5.2 EVM → Solana 测试
- [ ] 5.3 Solana → EVM 测试
- [ ] 5.4 性能测试
- [ ] 5.5 故障恢复测试

**交付物**:
- ✅ 完整的测试套件
- ✅ 性能报告
- ✅ 问题修复

---

## 📅 时间规划

| 阶段 | 开始日期 | 结束日期 | 状态 |
|------|---------|---------|------|
| 设计阶段 | 2025-11-05 | 2025-11-06 | ✅ 完成 |
| Phase 1 | 2025-11-07 | 2025-11-14 | 🚧 进行中 |
| Phase 2 | 2025-11-15 | 2025-11-22 | ⏳ 未开始 |
| Phase 3 | 2025-11-23 | 2025-11-29 | ⏳ 未开始 |
| Phase 4 | 2025-11-30 | 2025-12-06 | ⏳ 未开始 |
| Phase 5 | 2025-12-07 | 2025-12-13 | ⏳ 未开始 |

---

## 🎯 当前Sprint (本周)

**Sprint 目标**: 完成 Phase 1.1-1.2

**本周任务**:
1. ✅ 创建开发计划文档
2. 🚧 搭建 Docker 开发环境
3. ⏳ 开发 EVM Core Contract
4. ⏳ 编写合约测试

---

## 📈 进度追踪

### 2025-11-07 (今天)
- ✅ 创建开发计划文档
- 🚧 开始 Phase 1.1: Docker 环境搭建

---

## ⚠️ 风险与问题

| 风险 | 影响 | 缓解措施 | 状态 |
|------|------|---------|------|
| P2P 网络调试复杂 | 中 | 使用 Wormhole 已验证的 libp2p 配置 | 监控中 |
| Solana 账户设计 | 中 | 参考 Anchor 最佳实践 | 监控中 |
| 19 节点资源消耗 | 低 | 使用 Docker Compose 资源限制 | 已缓解 |

---

## 📚 相关文档

- [01-bridge-design.md](./01-bridge-design.md) - 桥接设计
- [02-implementation-plan.md](./02-implementation-plan.md) - 实施方案
- [03-technical-research.md](./03-technical-research.md) - 技术调研
- [04-system-design.md](./04-system-design.md) - 系统设计
- [05-quick-reference.md](./05-quick-reference.md) - 快速参考
- [06-digital-signature-primer.md](./06-digital-signature-primer.md) - 签名原理
- [07-consensus-and-relay.md](./07-consensus-and-relay.md) - 共识机制

---

**最后更新**: 2025-11-07  
**负责人**: 开发团队

