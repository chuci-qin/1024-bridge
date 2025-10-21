# Liquidity Mining (Phase 3)

> **Status**: Design phase, reserved interfaces in Phase 2  
> **Timeline**: To be implemented after Phase 2 stable (3-6 months)

---

## 🎯 Purpose

Incentivize liquidity providers (LPs) to ensure sufficient bridge liquidity.

---

## 💰 Economic Model

### Fee Structure

**Bridge Fee**: 0.1% per transaction

**Distribution** (Phase 3):
```
Total Fee: 0.1%
├─ 70% → LP Pool  ⭐
├─ 20% → Protocol Revenue
└─ 10% → Insurance Fund
```

**Phase 2**: Fees collected but not distributed (stored in contract)

---

### LP Rewards

**Sources**:
1. **Fee Sharing**: 70% of bridge fees
2. **Token Rewards**: Optional 1024 token emissions

**APY Calculation**:
```
Daily Volume: $1M
Daily Fees (0.1%): $1,000
LP Share (70%): $700/day

LP APY = ($700 * 365) / Total LP Staked
```

**Example**: With $100K total LP staked → ~256% APY

---

## 🔧 Technical Implementation

### Reserved Interfaces (Phase 2)

```solidity
// Already defined in 1024Bridge.sol Phase 2
mapping(address => uint256) public lpStakes;
uint256 public totalLPStaked;
uint256 public accumulatedFees;

function stakeLiquidity(uint256 amount) external {
    revert("Phase 3 feature");  // Stub
}

function unstakeLiquidity(uint256 amount) external {
    revert("Phase 3 feature");  // Stub
}
```

**Phase 3**: Simply implement these functions, no contract refactor needed ✅

---

### Phase 3 Implementation

**New Functions**:
```solidity
function stakeLiquidity(uint256 amount) external {
    USDC.transferFrom(msg.sender, address(this), amount);
    lpStakes[msg.sender] += amount;
    totalLPStaked += amount;
    updateRewards(msg.sender);
    emit LiquidityStaked(msg.sender, amount);
}

function claimRewards() external {
    uint256 rewards = calculateRewards(msg.sender);
    USDC.transfer(msg.sender, rewards);
    emit RewardsClaimed(msg.sender, rewards);
}
```

---

## 📊 Database Extensions (Phase 3)

### New Tables

```sql
-- LP staking records
CREATE TABLE bridge_lp_stakes (
    id UUID PRIMARY KEY,
    lp_address VARCHAR(42),
    staked_amount_e6 BIGINT,
    staked_at TIMESTAMPTZ,
    unstaked_at TIMESTAMPTZ,
    status VARCHAR(20)
);

-- Reward distribution
CREATE TABLE bridge_lp_rewards (
    id UUID PRIMARY KEY,
    lp_address VARCHAR(42),
    amount_e6 BIGINT,
    claimed_at TIMESTAMPTZ
);
```

---

## 🎨 Frontend (Phase 3)

### New Page: `/trading/bridge-liquidity`

**Features**:
- LP staking interface
- Rewards display
- APY calculator
- LP leaderboard

---

## 🛡️ Risk Management

### Minimum Liquidity Ratio

```solidity
// Ensure 20% liquidity always available
function canUnstake(uint256 amount) internal view returns (bool) {
    uint256 afterUnstake = getTotalLiquidity() - amount;
    uint256 minRequired = totalUserDeposits * 20 / 100;
    return afterUnstake >= minRequired;
}
```

---

## ✅ Extensibility Confirmed

**Phase 2 → Phase 3 Upgrade**:
- ✅ Zero breaking changes
- ✅ Reserved interfaces ready
- ✅ Fee collection already implemented
- ✅ Smooth upgrade path

---

**Full design**: See main documentation in 1024codebase repo

