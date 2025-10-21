# 1024 Bridge Architecture

> **Version**: Phase 2 (Basic Bridge)  
> **Last Updated**: 2025-10-21

---

## 🎯 Overview

1024 Bridge enables trustless cross-chain transfers between Arbitrum and 1024Chain (Solana).

**Key Understanding**: 1024Chain **IS** Solana blockchain (based on Agave fork).

---

## 🏗️ Two-Layer Architecture

### Complete Flow

```
User Assets (BTC/ETH/BNB/SOL)
  ↓
[ Layer 1: External Bridges (Wormhole/Stargate) ]
  User operated → Convert to Arbitrum USDC
  ↓
Arbitrum USDC (Liquidity Hub)
  ↓
[ Layer 2: 1024 Official Bridge ]← We Build This
  Arbitrum ↔ 1024Chain (Solana)
  ↓
User Account Balance on 1024 Platform
```

---

## 🔧 Components

### 1. Arbitrum Smart Contract

**Location**: `contracts/1024Bridge.sol`

**Functions**:
- `lockUSDC(amount, solanaAddress)` - Deposit
- `unlockUSDC(user, amount, burnTxHash)` - Withdraw

**Events**:
- `USDCLocked` - Emitted on deposit
- `USDCUnlocked` - Emitted on withdraw

---

### 2. Solana Bridge Program

**Type**: Anchor Program

**Instructions**:
- `mint_usdc(amount, arb_tx_hash)` - Mint USDC on Solana
- `burn_usdc(amount, arb_address)` - Burn USDC for withdraw

---

### 3. Bridge Relayer

**Location**: `1024-core/crates/bridge-relayer` (Main repo)

**Functions**:
- Listen to Arbitrum `USDCLocked` events → Execute mint on Solana
- Listen to Solana `burn_usdc` events → Execute unlock on Arbitrum

**Storage**: PostgreSQL (prevent replay attacks)

---

## 🔄 Deposit Flow

```
1. User calls lockUSDC on Arbitrum
   ↓
2. USDC transferred to contract
   ↓
3. USDCLocked event emitted
   ↓
4. Relayer detects event
   ↓
5. Relayer calls mint_usdc on Solana
   ↓
6. USDC minted to user's Solana address
   ↓
7. User receives USDC on 1024Chain ✅
```

**Time**: 3-5 minutes

**Fee**: 0.1% (Phase 2: collected, Phase 3: distributed to LPs)

---

## 🔄 Withdraw Flow

```
1. User calls burn_usdc on Solana
   ↓
2. USDC burned from user's account
   ↓
3. Event logged on-chain
   ↓
4. Relayer detects burn
   ↓
5. Relayer calls unlockUSDC on Arbitrum
   ↓
6. USDC transferred to user's Arbitrum address ✅
```

**Time**: 3-5 minutes

**Default Destination**: Arbitrum USDC (user can bridge further if needed)

---

## 🛡️ Security Design

### Access Control

```solidity
DEFAULT_ADMIN_ROLE → Contract deployment and config
RELAYER_ROLE → Execute unlockUSDC
GUARDIAN_ROLE → Emergency pause (3-of-5 MultiSig)
```

---

### Rate Limiting

```
Single Transaction: ≤ 100K USDC
Hourly Volume: ≤ 1M USDC
```

Prevents large-scale exploits.

---

### Replay Protection

**Arbitrum → Solana**:
- Solana Program tracks processed `arb_tx_hash`
- Each hash can only be processed once

**Solana → Arbitrum**:
- Arbitrum contract tracks processed `burnTxHash`
- Prevents double withdrawal

---

### Emergency Pause

**Trigger**: Security incident or anomaly

**Action**: Guardians (3-of-5 MultiSig) call `pause()`

**Effect**: All deposits and withdrawals frozen

**Resume**: Guardians call `unpause()` after issue resolved

---

## 🔮 Phase 3 Extensions

### Liquidity Mining (Reserved)

**Phase 2 (Current)**:
- Basic bridge functionality
- Fee collection (not distributed)

**Phase 3 (Future)**:
- LP staking enabled
- Fee distribution (70% to LPs)
- LP Token issuance
- APY display

**Contract Design**: Interfaces already reserved in Phase 2

See [`docs/LIQUIDITY-MINING.md`](LIQUIDITY-MINING.md)

---

## 📊 Gas Optimization

### Deposit

**Estimated Gas**: ~80,000 gas
**Cost (Arbitrum)**: ~$0.10 - $0.50 (depending on gas price)

### Withdraw

**Estimated Gas**: ~100,000 gas
**Cost (Arbitrum)**: ~$0.15 - $0.75

---

## 🧪 Testing Strategy

### Unit Tests

- All functions
- Edge cases
- Access control
- Error handling

### Integration Tests

- Full deposit flow (Arbitrum → Solana)
- Full withdraw flow (Solana → Arbitrum)
- Relayer failure recovery

### Security Tests

- Replay attacks
- Reentrancy
- Integer overflow
- Access control bypass

---

## 📚 Related Documentation

- **Security**: [SECURITY.md](../SECURITY.md)
- **Contributing**: [CONTRIBUTING.md](../CONTRIBUTING.md)
- **Liquidity Mining**: [LIQUIDITY-MINING.md](LIQUIDITY-MINING.md)

---

**For detailed implementation, see contract source code and inline comments.**

