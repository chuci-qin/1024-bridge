# 🌉 1024 Bridge

> **Official Bridge connecting Arbitrum and 1024Chain (Solana)**

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Solidity](https://img.shields.io/badge/Solidity-0.8.20-blue)](https://soliditylang.org/)
[![Hardhat](https://img.shields.io/badge/Hardhat-2.19-orange)](https://hardhat.org/)

---

## 🎯 Overview

1024 Bridge is a secure, efficient cross-chain bridge enabling seamless USDC transfers between Arbitrum and 1024Chain (Solana).

### Two-Layer Architecture

```
Any Chain Assets (BTC/ETH/BNB/SOL)
  ↓
[ Layer 1: External Bridges (Wormhole/Stargate) ]
  → User operated, convert to Arbitrum USDC
  ↓
Arbitrum USDC (Hub)
  ↓
[ Layer 2: 1024 Official Bridge ]← This Repository
  → Arbitrum ↔ 1024Chain (Solana)
  ↓
User Trading on 1024 Platform
```

**Why This Design?**
- ✅ Maintain only ONE bridge (vs multiple)
- ✅ Leverage mature infrastructure (Wormhole/Stargate)
- ✅ Concentrate liquidity on Arbitrum
- ✅ Low gas fees
- ✅ Save 200+ hours development time

---

## 📋 Features

### Phase 2 (Current)

- ✅ **Deposit**: Lock USDC on Arbitrum → Mint on 1024Chain (Solana)
- ✅ **Withdraw**: Burn on 1024Chain → Unlock on Arbitrum
- ✅ **Security**: MultiSig guardians, rate limiting, emergency pause
- ✅ **Scalability**: Reserved interfaces for liquidity mining (Phase 3)

### Phase 3 (Planned)

- 🔮 **Liquidity Mining**: LP staking with fee sharing
- 🔮 **LP Rewards**: 70% fee distribution + token rewards
- 🔮 **APY Display**: Real-time yield calculation

---

## 🚀 Quick Start

### Prerequisites

- Node.js >= 18
- npm or yarn
- Hardhat

### Installation

```bash
# Clone repository
git clone https://github.com/your-org/1024-bridge.git
cd 1024-bridge

# Install dependencies
npm install

# Compile contracts
npx hardhat compile

# Run tests
npx hardhat test
```

---

## 📁 Project Structure

```
1024-bridge/
├── contracts/
│   ├── 1024Bridge.sol       # Core bridge contract
│   └── MockUSDC.sol          # Test USDC contract
├── test/
│   └── 1024Bridge.test.js    # Test suite
├── scripts/
│   └── deploy.js             # Deployment script
├── docs/                     # Documentation
├── audits/                   # Security audit reports
└── README.md
```

---

## 🔧 Development

### Compile Contracts

```bash
npx hardhat compile
```

### Run Tests

```bash
npx hardhat test
```

### Local Testing

```bash
npx hardhat node
```

### Deploy to Testnet

```bash
# Arbitrum Sepolia
npx hardhat run scripts/deploy.js --network arbitrumSepolia

# Verify contract
npx hardhat verify --network arbitrumSepolia <CONTRACT_ADDRESS> <USDC_ADDRESS>
```

---

## 📊 Contract Architecture

### Core Functions

**Deposit (Arbitrum → 1024Chain)**:
```solidity
function lockUSDC(uint256 amount, string calldata solanaAddress) external
```
- Locks USDC on Arbitrum
- Emits `USDCLocked` event
- Relayer listens and mints on Solana

**Withdraw (1024Chain → Arbitrum)**:
```solidity
function unlockUSDC(address user, uint256 amount, bytes32 burnTxHash) external
```
- Called by Relayer
- Unlocks USDC to user's Arbitrum address
- Replay protection via `processedWithdrawals`

---

### Security Features

**Access Control**:
- `DEFAULT_ADMIN_ROLE` - Contract admin
- `RELAYER_ROLE` - Bridge relayer service
- `GUARDIAN_ROLE` - Emergency pause (3-of-5 MultiSig)

**Rate Limiting**:
- Single transaction limit: 100K USDC
- Hourly volume limit: 1M USDC

**Security Mechanisms**:
- Pausable (emergency stop)
- ReentrancyGuard
- Replay attack protection
- Withdrawal verification

---

### Extensibility (Phase 3)

**Reserved for Liquidity Mining**:
```solidity
// LP staking (Phase 3)
mapping(address => uint256) public lpStakes;
uint256 public totalLPStaked;
uint256 public accumulatedFees;

// Reserved functions
function stakeLiquidity(uint256 amount) external;
function unstakeLiquidity(uint256 amount) external;
function calculateLPRewards(address lp) public view returns (uint256);
```

**Bridge Fee**: 0.1%
- Phase 2: Collected but not distributed
- Phase 3: 70% to LPs, 20% protocol, 10% insurance

---

## 🧪 Testing

### Test Coverage

- ✅ Deposit functionality
- ✅ Withdraw functionality
- ✅ Rate limit enforcement
- ✅ Replay attack protection
- ✅ Access control
- ✅ Emergency pause

### Run Tests

```bash
npx hardhat test
```

### Test Report

```
  1024Bridge
    充值功能
      ✓ 应该成功锁定USDC
      ✓ 应该拒绝超过限额的充值
    提现功能
      ✓ Relayer应该可以解锁USDC
      ✓ 应该防止重放攻击
      ✓ 非Relayer不能解锁
    管理功能
      ✓ Guardian应该可以暂停
    LP功能（Phase 3预留）
      ✓ Phase 2应该revert LP功能

  7 passing
```

---

## 🔗 Networks

### Arbitrum Sepolia (Testnet)

- **RPC**: https://sepolia-rollup.arbitrum.io/rpc
- **Chain ID**: 421614
- **Explorer**: https://sepolia.arbiscan.io
- **Faucet**: https://faucet.quicknode.com/arbitrum/sepolia

### Arbitrum One (Mainnet)

- **RPC**: https://arb1.arbitrum.io/rpc
- **Chain ID**: 42161
- **Explorer**: https://arbiscan.io
- **USDC**: `0xaf88d065e77c8cC2239327C5EDb3A432268e5831`

---

## 🛡️ Security

### Audit Status

- ⏳ **Phase 2**: Pending audit
- 📋 **Auditors**: TBD

### Bug Bounty

- 🔍 **Program**: Coming soon
- 💰 **Rewards**: Up to $50,000

### Report a Vulnerability

Please email: security@1024.exchange

**Do not** open public issues for security vulnerabilities.

---

## 📚 Documentation

### Architecture

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)

### Security

See [`docs/SECURITY.md`](docs/SECURITY.md)

### Integration Guide

See [`docs/INTEGRATION.md`](docs/INTEGRATION.md)

---

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details.

### Development Workflow

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'feat: Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

## 🔗 Links

- **Website**: https://1024.exchange
- **Documentation**: https://docs.1024.exchange/bridge
- **Discord**: https://discord.gg/1024exchange
- **Twitter**: https://twitter.com/1024exchange

---

## ⚠️ Disclaimer

This software is provided "as is", without warranty of any kind. Use at your own risk.

Bridge contracts are under development and have not been audited yet. **DO NOT** use with real funds until official audit is complete.

---

## 🙏 Acknowledgments

- [OpenZeppelin](https://openzeppelin.com/) - Secure smart contract libraries
- [Hardhat](https://hardhat.org/) - Development environment
- [Wormhole](https://wormhole.com/) - Cross-chain infrastructure inspiration

---

**Built with ❤️ by 1024 Exchange Team**
