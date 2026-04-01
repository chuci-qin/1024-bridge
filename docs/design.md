# Bridge1024 Architecture Design

## Overview

Bridge1024 is a stake-unlock cross-chain bridge for USDC transfers between EVM chains (Ethereum, Arbitrum, Base) and SVM chains (Solana, 1024chain).

## Architecture

### Components

1. **EVM Contract** (`contracts/evm/src/Bridge1024.sol`)
   - Deployed to Arbitrum, Ethereum, Base (testnets + mainnets)
   - Handles stake (deposit) and submitSignature (unlock)
   - Multi-relayer threshold signatures (ECDSA, ceil(2/3))
   - Rate limiting, pausable, reentrancy protection

2. **SVM Program** (`contracts/svm/programs/bridge1024/src/lib.rs`)
   - Single program deployed to both Solana and 1024chain
   - Handles stake and submit_signature
   - Ed25519 signature verification via Solana precompile
   - Bridge fee collected only on 1024chain deployments

3. **Relayer** (`relayer/`)
   - Single Docker image with listener + submitter binaries
   - Listener: polls source chain for StakeEvents
   - Submitter: signs and submits to target chain
   - File-based event queue for reliability

### Security Model

#### Four-Layer Defense

1. **Cryptographic Verification**: Multi-relayer ceil(2/3) threshold
2. **Rate Limiting**: Sliding window limits on unlock volume
3. **Circuit Breaker**: Pausable operations for emergency response
4. **Admin Controls**: Two-step transfer, emergency withdraw

#### Key Security Features

- Nonce bitmap (not sequential) — prevents fund loss from out-of-order processing
- Unified 32-byte sender — prevents cross-chain data format mismatches
- Ed25519 instruction_index validation — prevents Wormhole-style bypass
- receiver_token_account validation — prevents fund theft
- USDC mint validation — prevents fake token deposit attacks
- Canonical ECDSA s-value — prevents signature malleability
- SafeERC20 with balance delta — prevents fee-on-transfer drain
- Token-2022 safety — balance checks before/after transfer

### Supported Chains

| Chain | Chain ID | Type | Environment |
|-------|----------|------|-------------|
| Arbitrum Sepolia | 421614 | EVM | Testnet |
| Ethereum Sepolia | 11155111 | EVM | Testnet |
| Base Sepolia | 84532 | EVM | Testnet |
| Solana Devnet | 103 | SVM | Testnet |
| Arbitrum One | 42161 | EVM | Mainnet |
| Ethereum Mainnet | 1 | EVM | Mainnet |
| Base Mainnet | 8453 | EVM | Mainnet |
| Solana Mainnet | 101 | SVM | Mainnet |
| 1024 Testnet | 91025 | SVM | Testnet |
| 1024 Stablenet | 91026 | SVM | Stablenet |
| 1024 Mainnet | 91024 | SVM | Mainnet |

### Fee Model

- EVM contracts: No bridge fee
- Solana program: bridge_fee = 0 (set at deployment)
- 1024chain program: bridge_fee > 0 (configurable, deducted from unlock amount)

### Data Flow

1. User calls `stake()` on source chain with USDC amount and receiver address
2. Contract locks USDC in vault and emits `StakeEvent`
3. Relayer listener detects event (HTTP polling for EVM, RPC polling for SVM)
4. Relayer writes event to file queue
5. Relayer submitter reads queue, signs event data, submits to target chain
6. Multiple relayers submit signatures independently
7. When ceil(2/3) threshold reached, target contract unlocks USDC to receiver
