Feature: 1024chain to EVM bridge
  As a user with USDC on 1024chain
  I want to bridge my USDC to an EVM chain
  So that I can use USDC on Ethereum/Arbitrum/Base

  Background:
    Given a configured bridge "arbsep-1024test-usdc"
    And 3 relayers are configured

  Scenario: Successful USDC withdrawal from 1024chain to Arbitrum
    Given the user has 100 USDC on 1024chain
    And the Arbitrum bridge contract has sufficient USDC
    When the user calls stake(100000000, "<arbitrum_receiver_address>") on 1024chain
    Then a StakeEvent is emitted on 1024chain
    And the relayer detects the event
    And the relayer submits ECDSA signature to Arbitrum
    And after 2/3 threshold the receiver unlocks USDC on Arbitrum

  Scenario: Bridge fee is deducted on 1024chain stake
    Given the bridge fee is set to 100000 (0.1 USDC) on 1024chain
    When the user stakes 100000000 (100 USDC) on 1024chain
    Then the StakeEvent amount is 99900000 (99.9 USDC, fee deducted)
    And 0.1 USDC fee stays in the 1024chain vault
