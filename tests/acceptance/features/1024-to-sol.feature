Feature: 1024chain to Solana bridge (SVM-to-SVM)
  As a user with USDC on 1024chain
  I want to bridge my USDC to Solana
  So that I can use USDC on Solana

  Background:
    Given a configured bridge "soldev-1024test-usdc"
    And both SVM programs are deployed

  Scenario: Successful USDC from 1024chain to Solana
    Given the user has 100 USDC on 1024chain
    When the user calls stake on the 1024chain bridge program
    Then a StakeEvent is emitted on 1024chain with fee deducted
    And the relayer submits Ed25519 signature to Solana
    And the receiver unlocks USDC on Solana
