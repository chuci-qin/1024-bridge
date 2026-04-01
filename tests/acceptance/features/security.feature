Feature: Bridge security protections

  Scenario: Rate limiting prevents rapid vault drain
    Given the rate limit is 500000 USDC per 1 hour window
    And 400000 USDC was unlocked 30 minutes ago
    When a valid unlock of 200000 USDC is attempted
    Then the transaction reverts with "Rate limit exceeded"
    And the vault balance is unchanged

  Scenario: Nonce bitmap allows out-of-order processing
    Given nonce 3 and nonce 5 both have valid signatures
    When nonce 5 reaches threshold first
    Then nonce 5 is unlocked successfully
    And nonce 3 can still be processed later

  Scenario: Malicious relayer cannot steal funds (Wormhole lesson)
    Given a valid unlock for nonce 7 targeting user Alice
    When the relayer passes a different receiver token account
    Then the SVM program rejects with "receiver mismatch"

  Scenario: Malicious relayer cannot deposit fake mint (SVM-C2)
    Given a configured bridge with real USDC mint
    When an attacker calls stake with a fake mint
    Then the program rejects with "USDC mint mismatch"

  Scenario: Ed25519 instruction_index bypass prevented (Wormhole)
    Given a relayer crafts Ed25519 ix with instruction_index != 0xFFFF
    When they submit the signature
    Then the program rejects with "Invalid signature"

  Scenario: Circuit breaker halts operations
    Given the bridge is operating normally
    When admin calls pause()
    Then stake() reverts with "Paused"
    And submitSignature() reverts with "Paused"
    When admin calls unpause()
    Then operations resume normally

  Scenario: Two-step admin transfer prevents lockout
    Given admin proposes newAdmin
    When someone other than newAdmin calls acceptAdmin()
    Then the transaction reverts
    When newAdmin calls acceptAdmin()
    Then admin is successfully transferred

  Scenario: Replay attack blocked by nonce bitmap
    Given nonce 1 has been successfully processed
    When an attacker replays nonce 1 with valid signatures
    Then the transaction reverts with "Already processed"

  Scenario: Vault balance invariant prevents over-drain
    Given the vault has 1000 USDC and minimum reserve is 100 USDC
    When an unlock of 950 USDC is attempted
    Then the transaction reverts with "Insufficient reserve"
    And the vault retains all 1000 USDC
