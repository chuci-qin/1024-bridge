# ============================================================================
# ATDD 验收测试：1024chain → EVM 跨链桥方向
#
# 本文件使用 Gherkin 语法描述从 1024chain 到 EVM 链（如 Arbitrum）的跨链桥
# 验收测试场景。覆盖 USDC 成功提现和手续费扣除两个核心场景。
# ============================================================================

Feature: 1024chain to EVM bridge
  As a user with USDC on 1024chain
  I want to bridge my USDC to an EVM chain
  So that I can use USDC on Ethereum/Arbitrum/Base

  # 背景：所有场景共享的前置条件
  # 配置名为 "arbsep-1024test-usdc" 的桥（Arbitrum Sepolia ↔ 1024chain 测试网，USDC 资产）
  # 配置 3 个 relayer 节点（满足 2/3 多签门槛要求）
  Background:
    Given a configured bridge "arbsep-1024test-usdc"
    And 3 relayers are configured

  # 场景一：从 1024chain 成功提现 USDC 到 Arbitrum 的完整流程
  # 流程：用户在 1024chain 端调用 stake 质押 USDC → 链上触发 StakeEvent →
  # relayer 监听到事件并生成 ECDSA 签名提交到 Arbitrum →
  # 收集到 2/3 以上签名后触发 EVM 端合约解锁，接收方获得 USDC
  Scenario: Successful USDC withdrawal from 1024chain to Arbitrum
    Given the user has 100 USDC on 1024chain
    And the Arbitrum bridge contract has sufficient USDC
    When the user calls stake(100000000, "<arbitrum_receiver_address>") on 1024chain
    Then a StakeEvent is emitted on 1024chain
    And the relayer detects the event
    And the relayer submits ECDSA signature to Arbitrum
    And after 2/3 threshold the receiver unlocks USDC on Arbitrum

  # 场景二：跨链桥手续费扣除验证
  # 验证质押时手续费被正确扣除：用户质押 100 USDC，扣除 0.1 USDC 手续费后，
  # StakeEvent 中的实际金额为 99.9 USDC，手续费留在 1024chain 的金库（vault）中
  Scenario: Bridge fee is deducted on 1024chain stake
    Given the bridge fee is set to 100000 (0.1 USDC) on 1024chain
    When the user stakes 100000000 (100 USDC) on 1024chain
    Then the StakeEvent amount is 99900000 (99.9 USDC, fee deducted)
    And 0.1 USDC fee stays in the 1024chain vault
