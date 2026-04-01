Feature: Solana to 1024chain bridge (SVM-to-SVM)
  As a user with USDC on Solana
  I want to bridge my USDC to 1024chain
  So that I can use USDC in the 1024 ecosystem

  Background:
    Given a configured bridge "soldev-1024test-usdc"
    And both SVM programs are deployed

  Scenario: Successful USDC from Solana to 1024chain
    Given the user has 100 USDC on Solana Devnet
    When the user calls stake on the Solana bridge program
    Then a StakeEvent is emitted on Solana
    And the relayer detects the event via SVM listener
    And the relayer submits Ed25519 signature to 1024chain
    And the receiver unlocks USDC on 1024chain

  Scenario: Solana bridge has no fee
    Given the Solana bridge fee is set to 0
    When the user stakes 100 USDC on Solana
    Then the full 100 USDC amount appears in the StakeEvent
