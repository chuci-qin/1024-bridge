# Solana Core Bridge - Testing Guide

## 概述

本项目是一个基于Wormhole架构的Solana-Ethereum跨链桥合约，使用Anchor框架开发。

## 测试结构

### 测试文件

- **tests/solana-core.ts**: 主测试文件，包含以下测试套件：
  - `initialize`: 测试桥初始化和guardian集合设置
  - `post_message`: 测试消息发布功能
  - `post_vaa`: 测试VAA（Verifiable Action Approval）提交和验证
  - `verify_vaa_signatures`: 测试签名验证
  - `Integration`: 完整的消息流集成测试

## 运行测试

### 前置条件

1. 安装依赖：
```bash
cd /workspace/programs/solana-core
yarn install
```

2. 构建程序：
```bash
anchor build
```

### 运行测试

```bash
# 运行完整测试（包括本地验证器）
anchor test

# 使用已运行的验证器
anchor test --skip-local-validator

# 跳过构建（如果已经构建过）
anchor test --skip-build --skip-local-validator
```

## 测试覆盖

### 1. Initialize 测试
- ✅ 正常初始化桥和guardian集合
- ✅ 验证guardian keys正确存储
- ✅ 测试空guardians列表被拒绝

### 2. Post Message 测试
- ✅ 发布消息并验证内容
- ✅ 序列号正确递增
- ✅ 多条消息按序处理

### 3. Post VAA 测试
- ✅ 提交有效VAA
- ✅ 验证VAA内容正确存储
- ✅ 无效VAA版本被拒绝
- ✅ 错误的guardian set被拒绝
- ✅ 签名数量不足被拒绝

### 4. Verify Signatures 测试
- ✅ 足够签名数量通过验证
- ✅ 签名不足被拒绝

### 5. 集成测试
- ✅ 完整的消息流：发布 → VAA生成 → VAA验证

## 合约修改说明

在测试开发过程中，对合约进行了以下修改以修复编译错误：

1. **post_message** 函数签名更新：
   - 添加了 `sequence: u64` 参数
   - 账户名从 `sequence` 改为 `sequence_account`
   - 添加了序列号验证逻辑

2. **PostMessage** 账户结构更新：
   - 添加了 `#[instruction(sequence: u64)]` 属性
   - 账户名更改以避免命名冲突

3. **错误码扩展**：
   - 添加了 `InvalidSequence` 错误码

## 故障排查

### 常见问题

1. **找不到IDL文件**
   - 确保运行了 `anchor build`
   - 检查 `target/idl/solana_core.json` 是否存在

2. **程序ID不匹配**
   - 确认 `lib.rs` 中的 `declare_id!` 与 `Anchor.toml` 中的程序ID一致
   - 重新部署后更新程序ID

3. **验证器未运行**
   - 启动本地验证器: `solana-test-validator`
   - 或使用 `anchor test` 自动启动

## 测试数据

测试中使用的示例Guardian地址（Ethereum格式，20字节）：
```
0x1111111111111111111111111111111111111111
0x2222222222222222222222222222222222222222
0x3333333333333333333333333333333333333333
```

## 下一步

- [ ] 添加真实的secp256k1签名验证测试
- [ ] 添加重放攻击防护测试
- [ ] 添加Guardian集合更新测试
- [ ] 性能和Gas优化测试
- [ ] 边界条件和模糊测试

## 参考

- [Anchor Framework Documentation](https://www.anchor-lang.com/)
- [Wormhole Protocol Whitepaper](https://wormhole.com/papers/WhitepaperV2.pdf)
- [Solana Program Library](https://spl.solana.com/)

