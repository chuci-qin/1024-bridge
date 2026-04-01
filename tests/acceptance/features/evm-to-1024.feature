Feature: EVM to 1024chain bridge
  As a user with USDC on an EVM chain
  I want to bridge my USDC to 1024chain
  So that I can use USDC in the 1024 ecosystem

  Background:
    Given a configured bridge "arbsep-1024test-usdc"
    And the EVM contract is deployed on Arbitrum Sepolia
    And the SVM program is deployed on 1024chain Testnet
    And 3 relayers are configured with 2/3 threshold

  Scenario: Successful USDC deposit from Arbitrum to 1024chain
    Given the user has 100 USDC on Arbitrum Sepolia
    And the 1024chain vault has sufficient USDC liquidity
    When the user calls stake(100000000, "<1024chain_receiver_address>")
    Then a StakeEvent is emitted with nonce 1 and amount 100000000
    And the user's USDC balance decreases by 100 USDC
    And the bridge contract balance increases by 100 USDC
    And the relayer detects the event within 30 seconds
    And the relayer submits Ed25519 signature to 1024chain
    And after 2/3 threshold the receiver unlocks USDC to the user
    And bridge fee is deducted on 1024chain side

  Scenario: Deposit with insufficient USDC balance
    Given the user has 0 USDC on Arbitrum Sepolia
    When the user calls stake(100000000, "<receiver>")
    Then the transaction reverts

  Scenario: Deposit when bridge is paused
    Given the admin has paused the EVM bridge
    When the user calls stake(100000000, "<receiver>")
    Then the transaction reverts with "Paused"

  Scenario: Deposit with invalid receiver address
    Given the user has 100 USDC
    When the user calls stake(100000000, "")
    Then the transaction reverts with "InvalidReceiverAddress"
