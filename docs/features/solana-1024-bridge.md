# Solana <-> 1024chain Bidirectional Cross-Chain Bridge

## Overview

Enable bidirectional USDC transfers between Solana and 1024chain:
- **Solana -> 1024chain**: User stakes on Solana, relayer submits to 1024chain, 1024chain unlocks to receiver
- **1024chain -> Solana**: User stakes on 1024chain, relayer submits to Solana, Solana unlocks to receiver

Both directions use the same stake-unlock-with-multisig pattern. Ed25519 signatures are used for relayer attestation.

Fee is **only** charged on the 1024chain (SVM) side. The Solana contract charges **zero fees**.

## Acceptance Criteria

### AC-SOL-01: Solana -> 1024chain Stake

**Given** a user with USDC on Solana and a configured Solana Bridge program
**When** the user calls `stake(amount, receiver_1024_address)` on the Solana Bridge
**Then**:
1. The full `amount` is transferred from the user's token account to the vault (no fee deduction)
2. A `StakeEvent` is emitted with `amount` (full amount), the user's Solana pubkey as sender
3. Nonce increments by 1

### AC-SOL-02: Solana -> 1024chain Unlock

**Given** a `StakeEvent` from Solana captured by the sol2svm relayer
**When** 2/3 of whitelisted relayers submit matching Ed25519-signed `submit_signature` to 1024chain
**Then**:
1. The 1024chain receiver unlocks `event_data.amount - bridge_fee` to the receiver address
2. `CrossChainSuccessEvent` is emitted on 1024chain

### AC-SOL-03: 1024chain -> Solana Stake

**Given** a user with USDC on 1024chain and a configured 1024chain Bridge program
**When** the user calls `stake(amount, receiver_solana_address)` on the 1024chain Bridge
**Then**:
1. The amount (minus 1024chain bridge fee) is emitted in `StakeEvent`
2. The relayer (svm2sol) picks up the event

### AC-SOL-04: 1024chain -> Solana Unlock

**Given** a `StakeEvent` from 1024chain captured by the svm2sol relayer
**When** 2/3 of whitelisted relayers submit matching Ed25519-signed `submit_signature` to the Solana contract
**Then**:
1. The Solana receiver unlocks `event_data.amount` to the receiver address (no fee on Solana side)
2. `CrossChainSuccessEvent` is emitted on Solana

### AC-SOL-05: Nonce and Replay Protection

**Given** the bridge contracts on both chains
**When** a relayer attempts to submit a signature with a nonce <= last_nonce
**Then** the transaction is rejected with `InvalidNonce`

### AC-SOL-06: Relayer Whitelist

**Given** the Solana receiver contract with configured relayers
**When** a non-whitelisted address attempts `submit_signature`
**Then** the transaction is rejected with `Unauthorized`

### AC-SOL-07: Zero Fee on Solana

**Given** the Solana bridge contract
**When** a user stakes any amount
**Then** the `StakeEvent.amount` equals the full staked amount (no fee deducted)

### AC-SOL-08: 32-Byte Sender Compatibility

**Given** the unified 32-byte sender field in `StakeEventData`
**When** an EVM->1024chain transfer occurs (20-byte EVM address, zero-padded to 32 bytes)
**Then** the existing E2S flow continues to work correctly
**And** a Solana->1024chain transfer uses the full 32-byte Solana pubkey as sender

## Test Scenarios

### Unified Contract Tests

| ID | Scenario | Expected |
|----|----------|----------|
| SOL-T001 | Initialize bridge program | sender_state and receiver_state created with defaults |
| SOL-T002 | configure_usdc sets USDC mint | Both sender_state and receiver_state updated |
| SOL-T003 | configure_peer sets target chain | sender_state.target_contract and receiver_state.source_contract set |

### Sender Tests (Solana -> 1024chain)

| ID | Scenario | Expected |
|----|----------|----------|
| SOL-T004 | stake() transfers tokens to vault | Full amount transferred, no fee |
| SOL-T005 | stake() emits StakeEvent | StakeEvent.amount = full amount |
| SOL-T006 | stake() increments nonce | nonce N -> N+1 |
| SOL-T007 | stake() rejects insufficient balance | Error |
| SOL-T008 | stake() rejects unauthorized user | Error |

### Receiver Tests (1024chain -> Solana)

| ID | Scenario | Expected |
|----|----------|----------|
| SOL-T101 | addRelayer with admin | relayer_count increments |
| SOL-T102 | removeRelayer with admin | relayer_count decrements |
| SOL-T103 | addRelayer/removeRelayer with non-admin | Rejected |
| SOL-T104 | submitSignature single relayer (below threshold) | Accepted but no unlock |
| SOL-T105 | submitSignature reaches 2/3 threshold | Tokens unlocked, CrossChainSuccessEvent emitted |
| SOL-T106 | submitSignature nonce replay | Rejected |
| SOL-T107 | submitSignature invalid signature | Rejected |
| SOL-T108 | submitSignature non-whitelisted relayer | Rejected |
| SOL-T109 | submitSignature USDC not configured | Rejected |
| SOL-T110 | submitSignature wrong source contract | Rejected |
| SOL-T111 | submitSignature wrong chain ID | Rejected |

### Security Tests

| ID | Scenario | Expected |
|----|----------|----------|
| SOL-ST001 | Nonce replay defense | Same/smaller nonce rejected |
| SOL-ST002 | Forged signature defense | Invalid signature rejected |
| SOL-ST003 | Permission control | Non-admin operations rejected |
| SOL-ST004 | Vault security | Direct vault transfer and over-unlock prevented |
| SOL-ST005 | Forged event defense | Wrong contract/chain ID rejected, PDA isolation works |

### Integration Tests

| ID | Scenario | Expected |
|----|----------|----------|
| SOL-IT001 | E2E Solana stake -> simulated relayer -> Solana receiver unlock | Full cycle works |
| SOL-IT002 | E2E with 32-byte sender (Solana vs EVM) | Both sender formats work |

## Architecture

```
=== Solana -> 1024chain ===

Solana User
    |
    v stake(amount, receiver_1024_address)
Solana Bridge Program (sender side)
    |  - Lock full USDC amount in vault (NO fee)
    |  - Emit StakeEvent(amount, sender=solana_pubkey)
    |
    v StakeEvent
sol2svm-listener (Rust, polling Solana)
    |  - Parse Anchor StakeEvent from logs
    |  - Write StakeEventData to file queue
    |
    v queue file
sol2svm-submitter (Rust)
    |  - Convert to CompactStakeEventData (32B sender)
    |  - Ed25519 sign + submit_signature to 1024chain
    |
    v submit_signature
1024chain Receiver (svm/bridge1024)
    |  - Verify Ed25519 signatures
    |  - Threshold check (2/3)
    |  - Deduct bridge_fee, unlock net USDC to receiver
    v
1024chain User receives USDC


=== 1024chain -> Solana ===

1024chain User
    |
    v stake(amount, receiver_solana_address)
1024chain Bridge Program (sender side)
    |  - Deduct bridge_fee from amount
    |  - Emit StakeEvent(net_amount, sender=1024_pubkey)
    |
    v StakeEvent
svm2sol-listener (Rust, WebSocket to 1024chain)
    |  - Parse Anchor StakeEvent from logs
    |  - Write StakeEventData to file queue
    |
    v queue file
svm2sol-submitter (Rust)
    |  - Convert to CompactStakeEventData (32B sender)
    |  - Ed25519 sign + submit_signature to Solana
    |
    v submit_signature
Solana Bridge Program (receiver side)
    |  - Verify Ed25519 signatures
    |  - Threshold check (2/3)
    |  - Unlock full amount to receiver (NO fee on Solana)
    v
Solana User receives USDC
```
