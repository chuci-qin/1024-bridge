# Relayer 文档索引

| 文档                                   | 用途                                                                         |
| -------------------------------------- | ---------------------------------------------------------------------------- |
| [`architecture.md`](./architecture.md) | 整体执行模型：poller / submitter 解耦、pipelined submit、reorg 抗性、文件状态机、目录布局 |
| [`audit-log.md`](./audit-log.md)       | 历次审计发现与决策记录（按严重等级归类，每条带根因 + 修复方案 + 涉及文件）   |

外围参考：

- [`../.env.example`](../.env.example) — 全部支持的环境变量样例与说明
- [`../Cargo.toml`](../Cargo.toml) — 依赖清单
- [`../Dockerfile`](../Dockerfile) — 部署镜像（DATA_DIR=/data，VOLUME 持久化）
- [`../../docs/design.md`](../../docs/design.md) — 整个 1024-bridge 的合约 + relayer 顶层设计
