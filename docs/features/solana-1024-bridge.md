# Solana → 1024chain Cross-Chain Bridge

## Overview

Enable cross-chain USDC transfers from Solana to 1024chain (1024ex), following the same stake-unlock-with-multisig pattern used by the existing EVM → 1024chain bridge.

## Acceptance Criteria

### AC-1: Normal Cross-Chain Transfer (Solana → 1024chain)

**Given** a user with USDC on Solana and a configured Solana Bridge program  
**When** the user calls `stake(amount, receiver_1024_address)` on the Solana Bridge  
**Then**:
1. The full `amount` is transferred from the user's token account to the Solana Bridge vault
2. A `StakeEvent` is emitted with `amount = net_amount` (i.e. `amount - bridge_fee`), and the user's Solana pubkey as sender
3. The relayer (sol2svm) picks up the event and constructs a `StakeEventData` with 32-byte sender
4. The relayer submits an Ed25519-signed `submit_signature` to the 1024chain receiver
5. Once 2/3 of relayers have submitted matching signatures, the 1024chain receiver unlocks USDC to `receiver_1024_address`

### AC-2: Bridge Fee Deduction

**Given** a `bridge_fee` of N USDC configured on the Solana Bridge program  
**When** the user stakes `amount` USDC  
**Then**:
- The `StakeEvent.amount` equals `amount - bridge_fee` (net amount)
- The fee remains in the Solana Bridge vault as protocol revenue
- The 1024chain receiver unlocks `event_data.amount` to the receiver (no double fee)

### AC-3: Bridge Fee = 0

**Given** `bridge_fee = 0` on the Solana Bridge program  
**When** the user stakes `amount` USDC  
**Then** the `StakeEvent.amount` equals `amount` (full amount passes through)

### AC-4: Nonce Incrementing

**Given** the Solana Bridge sender state with current nonce = N  
**When** the user calls `stake()`  
**Then** nonce becomes N+1 and the `StakeEvent.nonce` equals N+1

### AC-5: Relayer Threshold Not Met

**Given** only 1 out of 3 relayers has submitted a signature for a given nonce  
**When** no more relayers submit  
**Then** tokens remain locked on 1024chain; no unlock occurs

### AC-6: Invalid Receiver Address

**Given** a receiver address that is not a valid 1024chain address  
**When** the relayer attempts to submit the signature to 1024chain  
**Then** the transaction fails with an appropriate error

### AC-7: 32-Byte Sender Compatibility

**Given** the 1024chain receiver now accepts 32-byte sender fields  
**When** an EVM→1024chain transfer occurs (20-byte EVM address, zero-padded to 32 bytes)  
**Then** the existing E2S flow continues to work correctly

## Test Scenarios

### Unit Tests (Solana Bridge Program)

| ID | Scenario | Expected |
|----|----------|----------|
| SOL-T001 | Initialize bridge program | sender_state and receiver_state created with defaults |
| SOL-T002 | configure_usdc sets USDC mint | sender_state.usdc_mint updated |
| SOL-T003 | configure_peer sets target chain | sender_state.target_contract, chain_ids set |
| SOL-T004 | configure_fee sets bridge_fee | receiver_state.bridge_fee updated |
| SOL-T005 | stake() transfers tokens to vault | user balance decreases, vault balance increases |
| SOL-T006 | stake() emits StakeEvent with net_amount | StakeEvent.amount = amount - fee |
| SOL-T007 | stake() increments nonce | nonce goes from N to N+1 |
| SOL-T008 | stake() with fee=0 emits full amount | StakeEvent.amount = amount |
| SOL-T009 | stake() fails when USDC not configured | Error: UsdcNotConfigured |
| SOL-T010 | stake() with amount <= fee emits 0 | StakeEvent.amount = 0 |

### Unit Tests (1024chain Receiver - 32-byte sender)

| ID | Scenario | Expected |
|----|----------|----------|
| RCV-T001 | submit_signature with 32-byte Solana sender | Unlocks correctly |
| RCV-T002 | submit_signature with zero-padded 20-byte EVM sender | Backward-compatible unlock |
| RCV-T003 | CrossChainSuccessEvent.sender_address field correct | 32-byte sender rendered properly |

### Unit Tests (Relayer shared types)

| ID | Scenario | Expected |
|----|----------|----------|
| REL-T001 | CompactStakeEventData with 32-byte sender serializes correctly | Borsh output matches on-chain format |
| REL-T002 | to_compact() for EVM address zero-pads to 32 bytes | First 12 bytes are 0x00 |
| REL-T003 | to_compact() for Solana address fills 32 bytes directly | Full pubkey preserved |

### End-to-End Tests

| ID | Scenario | Expected |
|----|----------|----------|
| E2E-SOL-001 | Solana stake → relayer → 1024chain unlock | Receiver balance increases by net_amount |
| E2E-SOL-002 | EVM stake → relayer → 1024chain unlock (regression) | Existing E2S still works with 32B sender |

## Architecture

```
Solana User
    │
    ▼ stake(amount, receiver_1024_address)
Solana Bridge Program (solana/bridge1024)
    │  - Lock USDC in vault
    │  - Deduct bridge_fee → emit StakeEvent(net_amount)
    │
    ▼ StakeEvent
sol2svm-listener (Rust)
    │  - Poll Solana signatures / logs
    │  - Parse Anchor StakeEvent
    │  - Write to file queue
    │
    ▼ queue file
sol2svm-submitter (Rust)
    │  - Read queue
    │  - Build StakeEventData (32B sender = Solana Pubkey)
    │  - Ed25519 sign + submit_signature to 1024chain
    │
    ▼ submit_signature
1024chain Receiver (svm/bridge1024)
    │  - Verify Ed25519 signatures
    │  - Threshold check (2/3)
    │  - Unlock USDC to receiver_address
    ▼
1024chain User receives USDC
```
