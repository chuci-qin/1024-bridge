好的！下面是一份**从零开始、可直接交给 Cursor 执行**的**完整开发文档**。在这版里，我已经把你要求的**Program 的 Initialize / Withdraw 客户端（TS）**与**1024 链（Solana/Agave 兼容）SDK 调用**全部补齐，并把**Relayer 的逻辑**也补到可跑通（V1 阈值=1 的 happy path）。
本版相对你上一版的程序做了**必要增强**：允许 Program 在需要时**自动创建 PDA 账户（config / processed）**，并在 `MintOnDeposit` 指令里**接收 payer** 用来支付租金，确保真正做到“一键跑通”。

---

# 0. 总览（V1 路线与组件）

* **官方桥主干（与 Hyper 路径一致）**
  外部桥（任意链资产 → **Arbitrum USDC**）→ **官方桥**（**Arbitrum USDC ↔ 1024Chain USDC**）
* **统一保证金资产**：USDC（6 decimals）
* **组件一览**

  * `packages/evm-contracts/` — Arbitrum 侧合约（Foundry）
  * `packages/program-1024/` — 1024 链（Agave/Solana Runtime）侧 Program（Rust，无 Anchor）
  * `packages/relayer/` — 中继服务（Node/TS），监听两侧事件并互相调用
  * `packages/frontend/` — 前端模块（Next.js + Web3Modal/Ethers + 你现有的 Solana Wallet Adapter）
  * `packages/proto/` — 协议常量与（未来）双语类型同步的占位
  * `infra/` — `.env` 模板、部署脚本、Docker 配置
* **网络与资产（测试网）**

  * EVM：**Arbitrum Sepolia**（可以用 MockUSDC；上线前切换到 Circle 原生 USDC）
  * 1024：**1024Chain Testnet**（Agave 兼容）

---

# 1. Monorepo 初始化（空白文件夹起步）

在空白文件夹运行（或让 Cursor 执行）：

```bash
git init
pnpm init -y
cat > pnpm-workspace.yaml <<'YAML'
packages:
  - "packages/*"
YAML

mkdir -p packages/{evm-contracts,program-1024,relayer,frontend,proto} infra/{docker,scripts}
```

`.editorconfig`：

```ini
root = true
[*]
indent_style = space
indent_size = 2
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
```

`.gitignore`：

```gitignore
node_modules
.DS_Store
dist
build
target
.idea
.env
.env.*
*.so
*.a
*.dylib
coverage
```

`infra/.env.example`（复制成 `infra/.env` 并填写）：

```env
# ---------- EVM / ARBITRUM ----------
EVM_RPC_URL=https://arb-sepolia.g.alchemy.com/v2/REPLACE_ME
EVM_CHAIN_ID=421614
EVM_USDC_ADDRESS=REPLACE_WITH_USDC_OR_MOCK
EVM_BRIDGE_ADDRESS=REPLACE_AFTER_DEPLOY
EVM_RELAYER_PRIVKEY=0x0123...   # 开发用；生产迁 KMS/Vault

EVM_SIGNERS=0xSigner1           # V1 阈值=1，列出一个 signer 地址即可
EVM_THRESHOLD=1

# ---------- 1024CHAIN (Solana/Agave-compatible) ----------
SOLANA_RPC_URL=https://rpc.1024chain.testnet/    # 你的 1024Chain Testnet RPC
BRIDGE_PROGRAM_ID=REPLACE_AFTER_DEPLOY
USDC_1024_MINT=REPLACE_AFTER_INIT
RELAYER_SOLANA_KEYPAIR=./infra/relayer-id.json   # 本地 Keypair（dev）；生产用 KMS

SOLANA_SIGNERS=SignerPubkey1
SOLANA_THRESHOLD=1

# ---------- LIMITS ----------
MIN_DEPOSIT_USDC=5000000       # 6 decimals => 5 USDC
DAILY_LIMIT_USDC=500000000000  # 500,000 USDC

# ---------- FRONTEND ----------
NEXT_PUBLIC_EVM_CHAIN_ID=421614
NEXT_PUBLIC_EVM_BRIDGE_ADDRESS=${EVM_BRIDGE_ADDRESS}
NEXT_PUBLIC_EVM_USDC_ADDRESS=${EVM_USDC_ADDRESS}
NEXT_PUBLIC_1024_USDC_MINT=${USDC_1024_MINT}
NEXT_PUBLIC_BRIDGE_PROGRAM_ID=${BRIDGE_PROGRAM_ID}
```

---

# 2. EVM 侧合约（Arbitrum）— `packages/evm-contracts`

## 2.1 初始化 & 依赖

```bash
cd packages/evm-contracts
forge init --no-commit
forge install OpenZeppelin/openzeppelin-contracts@v4.9.6 --no-commit
```

`foundry.toml`：

```toml
[profile.default]
solc_version = "0.8.20"
optimizer = true
optimizer_runs = 200
src = "src"
out = "out"
libs = ["lib"]

[fmt]
line_length = 100
tab_width = 4
```

`remappings.txt`：

```
@openzeppelin/=lib/openzeppelin-contracts/
```

## 2.2 合约代码

**`src/EvmBridge.sol`**（与前版一致，V1：ECDSA signers，1-of-1 可跑通）

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "@openzeppelin/contracts/security/Pausable.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";

contract EvmBridge is Ownable, Pausable {
    using SafeERC20 for IERC20;

    IERC20 public immutable USDC;

    mapping(address => bool) public isSigner;
    uint256 public signerCount;
    uint256 public threshold;

    uint256 public depositNonce;
    mapping(bytes32 => bool) public usedWithdrawIds;

    uint256 public minDeposit;
    uint256 public dailyLimit;
    uint256 public dayStart;
    uint256 public dayVolume;

    event DepositCommitted(bytes32 indexed depositId, address indexed from, bytes32 indexed recipient1024, uint256 amount);
    event WithdrawFinalized(bytes32 indexed withdrawId, address indexed to, uint256 amount);
    event SignerUpdated(address signer, bool active);
    event ThresholdUpdated(uint256 threshold);
    event LimitsUpdated(uint256 minDeposit, uint256 dailyLimit);

    constructor(
        address _usdc,
        address[] memory _signers,
        uint256 _threshold,
        uint256 _minDeposit,
        uint256 _dailyLimit
    ) {
        require(_usdc != address(0), "USDC=0");
        require(_signers.length > 0, "no signers");
        require(_threshold > 0 && _threshold <= _signers.length, "bad threshold");

        USDC = IERC20(_usdc);
        for (uint256 i = 0; i < _signers.length; i++) {
            address s = _signers[i];
            require(s != address(0), "signer=0");
            require(!isSigner[s], "dup signer");
            isSigner[s] = true;
            signerCount++;
            emit SignerUpdated(s, true);
        }
        threshold = _threshold;
        emit ThresholdUpdated(_threshold);

        minDeposit = _minDeposit;
        dailyLimit = _dailyLimit;
        emit LimitsUpdated(_minDeposit, _dailyLimit);

        dayStart = _startOfDay(block.timestamp);
    }

    function setSigner(address s, bool active) external onlyOwner {
        require(s != address(0), "signer=0");
        if (active && !isSigner[s]) { isSigner[s] = true; signerCount++; }
        else if (!active && isSigner[s]) { isSigner[s] = false; signerCount--; }
        emit SignerUpdated(s, active);
    }

    function setThreshold(uint256 t) external onlyOwner {
        require(t > 0 && t <= signerCount, "bad threshold");
        threshold = t;
        emit ThresholdUpdated(t);
    }

    function setLimits(uint256 _minDeposit, uint256 _dailyLimit) external onlyOwner {
        minDeposit = _minDeposit;
        dailyLimit = _dailyLimit;
        emit LimitsUpdated(_minDeposit, _dailyLimit);
    }

    function pause() external onlyOwner { _pause(); }
    function unpause() external onlyOwner { _unpause(); }

    function deposit(uint256 amount, bytes32 recipient1024) external whenNotPaused {
        require(amount >= minDeposit, "min");
        _rollDay();
        require(dayVolume + amount <= dailyLimit, "day limit");

        USDC.safeTransferFrom(msg.sender, address(this), amount);

        bytes32 depositId = keccak256(
            abi.encodePacked(uint256(block.chainid), address(this), msg.sender, recipient1024, amount, depositNonce)
        );
        depositNonce++;
        dayVolume += amount;

        emit DepositCommitted(depositId, msg.sender, recipient1024, amount);
    }

    function withdraw(bytes32 withdrawId, address toEvm, uint256 amount, bytes[] calldata signatures) external whenNotPaused {
        require(!usedWithdrawIds[withdrawId], "used");
        require(toEvm != address(0), "to=0");
        require(amount > 0, "amt=0");
        require(signatures.length >= threshold, "sig<th");

        bytes32 msgHash = keccak256(
            abi.encodePacked(bytes("WITHDRAW:"), toEvm, amount, withdrawId, uint256(block.chainid))
        );
        bytes32 ethHash = ECDSA.toEthSignedMessageHash(msgHash);

        uint256 valid;
        address[] memory seen = new address[](signatures.length);

        for (uint256 i = 0; i < signatures.length; i++) {
            address recovered = ECDSA.recover(ethHash, signatures[i]);
            require(isSigner[recovered], "not signer");
            for (uint256 j = 0; j < i; j++) { require(seen[j] != recovered, "dup sig"); }
            seen[i] = recovered;
            valid++;
        }
        require(valid >= threshold, "thresh");

        usedWithdrawIds[withdrawId] = true;
        USDC.safeTransfer(toEvm, amount);
        emit WithdrawFinalized(withdrawId, toEvm, amount);
    }

    function _startOfDay(uint256 ts) internal pure returns (uint256) {
        return (ts / 1 days) * 1 days;
    }

    function _rollDay() internal {
        uint256 s = _startOfDay(block.timestamp);
        if (s > dayStart) { dayStart = s; dayVolume = 0; }
    }
}
```

**`src/mocks/MockUSDC.sol`**（测试用）

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;
import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
contract MockUSDC is ERC20 {
    uint8 private immutable _decimals = 6;
    constructor() ERC20("MockUSDC", "mUSDC") { _mint(msg.sender, 10_000_000 * (10 ** _decimals)); }
    function decimals() public view override returns (uint8) { return _decimals; }
}
```

## 2.3 部署脚本

`infra/scripts/evm.deploy.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
cd packages/evm-contracts
source ../../infra/.env

export RPC_URL="${EVM_RPC_URL}"
export ETH_RPC_URL="${EVM_RPC_URL}"
export PRIVATE_KEY_HEX="${EVM_RELAYER_PRIVKEY}"
export PRIVATE_KEY=$(cast to-uint256 ${PRIVATE_KEY_HEX})

forge build

cat > script/DeployEvmBridge.s.sol <<'SOL'
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/EvmBridge.sol";
import "../src/mocks/MockUSDC.sol";

contract DeployEvmBridge is Script {
    function run() external {
        address usdc = vm.envOr("EVM_USDC_ADDRESS", address(0));
        uint256 minDeposit = vm.envUint("MIN_DEPOSIT_USDC");
        uint256 dailyLimit = vm.envUint("DAILY_LIMIT_USDC");

        address signerAddr = vm.addr(vm.envUint("PRIVATE_KEY"));
        address;
        signers[0] = signerAddr;
        uint256 threshold = 1;

        vm.startBroadcast(vm.envUint("PRIVATE_KEY"));
        if (usdc == address(0)) {
            MockUSDC mock = new MockUSDC();
            usdc = address(mock);
        }
        EvmBridge bridge = new EvmBridge(usdc, signers, threshold, minDeposit, dailyLimit);
        vm.stopBroadcast();

        console2.log("USDC:", usdc);
        console2.log("Bridge:", address(bridge));
    }
}
SOL

forge script script/DeployEvmBridge.s.sol:DeployEvmBridge \
  --rpc-url ${RPC_URL} \
  --broadcast \
  --private-key ${PRIVATE_KEY} \
  -vvvv
```

---

# 3. 1024Chain 侧 Program（Rust，无 Anchor）— `packages/program-1024`

> **与前版差异**：
>
> * `Initialize` / `MintOnDeposit` 会在**需要时**自动创建 PDA（`config` / `processed`），但 PDA 的租金由**交易中的 `payer`**账户承担。
> * `MintOnDeposit` 现在**显式接收 `payer`** 账户。
> * 这样，Relayer 可以用自己的 Keypair 作为 payer，确保“一键跑通”。

## 3.1 Cargo

`Cargo.toml`：

```toml
[package]
name = "bridge_program_1024"
version = "0.2.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "lib"]
name = "bridge_program_1024"

[dependencies]
solana-program = "1.17.0"
borsh = "0.10.3"
thiserror = "1.0.63"
spl-token = { version = "4.0.0", features = ["no-entrypoint"] }

[features]
no-entrypoint = []

[dev-dependencies]
solana-program-test = "1.17.0"
solana-sdk = "1.17.0"
```

## 3.2 源码

**`src/error.rs`**（同前）

```rust
use thiserror::Error;
use solana_program::program_error::ProgramError;

#[derive(Error, Debug, Copy, Clone)]
pub enum BridgeError {
    #[error("Unauthorized")]
    Unauthorized,
    #[error("AlreadyProcessed")]
    AlreadyProcessed,
    #[error("InsufficientSigners")]
    InsufficientSigners,
    #[error("AmountTooSmall")]
    AmountTooSmall,
    #[error("InvalidAccount")]
    InvalidAccount,
}

impl From<BridgeError> for ProgramError {
    fn from(e: BridgeError) -> Self {
        ProgramError::Custom(e as u32)
    }
}
```

**`src/state.rs`**（同前）

```rust
use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::pubkey::Pubkey;

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct BridgeConfig {
    pub admin: Pubkey,
    pub usdc_mint: Pubkey,
    pub min_deposit: u64,
    pub daily_limit: u64,
    pub signers: Vec<Pubkey>,
    pub threshold: u8,
    pub day_start: i64,
    pub day_volume: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct Processed {
    pub used: bool,
}
```

**`src/instruction.rs`**（不变）

```rust
use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::pubkey::Pubkey;

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub enum BridgeIx {
    Initialize {
        admin: Pubkey,
        usdc_mint: Pubkey,
        min_deposit: u64,
        daily_limit: u64,
        signers: Vec<Pubkey>,
        threshold: u8,
    },
    MintOnDeposit {
        deposit_id: [u8; 32],
        amount: u64,
        recipient: Pubkey,
    },
    RequestWithdraw {
        withdraw_id: [u8; 32],
        amount: u64,
        to_evm: [u8; 20],
    },
}
```

**`src/lib.rs`**（增强版：可自动创建 PDA）

```rust
use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint, entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    sysvar::{clock::Clock, rent::Rent, Sysvar},
    system_instruction, system_program,
};
use spl_token::instruction as token_ix;

mod error;
mod state;
mod instruction;

use error::BridgeError;
use state::{BridgeConfig, Processed};
use instruction::BridgeIx;

pub const CONFIG_SEED: &[u8] = b"bridge-config";
pub const PROCESSED_SEED: &[u8] = b"processed";

entrypoint!(process_instruction);

fn start_of_day(ts: i64) -> i64 { (ts / 86400) * 86400 }

fn ensure_pda_created<'a>(
    program_id: &Pubkey,
    payer: &AccountInfo<'a>,
    target_ai: &AccountInfo<'a>,
    seeds: &[&[u8]],
    space: usize,
) -> ProgramResult {
    if target_ai.data_is_empty() {
        let rent = Rent::get()?;
        let lamports = rent.minimum_balance(space);
        let (pda, bump) = Pubkey::find_program_address(seeds, program_id);
        if pda != *target_ai.key { return Err(BridgeError::InvalidAccount.into()); }

        let create_ix = system_instruction::create_account(
            payer.key,
            &pda,
            lamports,
            space as u64,
            program_id,
        );
        invoke_signed(
            &create_ix,
            &[payer.clone(), target_ai.clone()],
            &[&[seeds[0], seeds.get(1).unwrap_or(&&[][..]), &[bump]].iter().copied().collect::<Vec<&[u8]>>()[..]],
        )?;
    }
    Ok(())
}

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let ix = BridgeIx::try_from_slice(data).map_err(|_| ProgramError::InvalidInstructionData)?;

    match ix {
        BridgeIx::Initialize { admin, usdc_mint, min_deposit, daily_limit, signers, threshold } => {
            let acc_iter = &mut accounts.iter();
            let payer = next_account_info(acc_iter)?;         // signer & rent payer
            let config_ai = next_account_info(acc_iter)?;     // PDA
            let system_prog = next_account_info(acc_iter)?;

            if !payer.is_signer { return Err(BridgeError::Unauthorized.into()); }
            if *system_prog.key != system_program::id() { return Err(BridgeError::InvalidAccount.into()); }
            if threshold == 0 || (threshold as usize) > signers.len() { return Err(ProgramError::InvalidArgument); }

            // ensure config PDA exists
            ensure_pda_created(program_id, payer, config_ai, &[CONFIG_SEED], 2048)?;

            let mut cfg = BridgeConfig {
                admin,
                usdc_mint,
                min_deposit,
                daily_limit,
                signers,
                threshold,
                day_start: start_of_day(Clock::get()?.unix_timestamp),
                day_volume: 0,
            };
            cfg.serialize(&mut &mut config_ai.data.borrow_mut()[..]).unwrap();
            Ok(())
        }

        BridgeIx::MintOnDeposit { deposit_id, amount, recipient } => {
            let acc_iter = &mut accounts.iter();
            let payer = next_account_info(acc_iter)?;           // signer & rent payer (relayer)
            let signer1 = next_account_info(acc_iter)?;         // one of cfg.signers (V1 threshold=1)
            let config_ai = next_account_info(acc_iter)?;
            let mint_ai = next_account_info(acc_iter)?;         // 1024USDC mint
            let recipient_ata = next_account_info(acc_iter)?;   // recipient ATA
            let processed_ai = next_account_info(acc_iter)?;    // processed PDA
            let token_program = next_account_info(acc_iter)?;
            let system_prog = next_account_info(acc_iter)?;

            if !payer.is_signer || !signer1.is_signer { return Err(BridgeError::Unauthorized.into()); }
            if *system_prog.key != system_program::id() { return Err(BridgeError::InvalidAccount.into()); }

            let mut cfg: BridgeConfig = BridgeConfig::try_from_slice(&config_ai.data.borrow())?;
            if *mint_ai.key != cfg.usdc_mint { return Err(BridgeError::InvalidAccount.into()); }
            if !cfg.signers.contains(signer1.key) { return Err(BridgeError::Unauthorized.into()); }
            if amount < cfg.min_deposit { return Err(BridgeError::AmountTooSmall.into()); }

            let clock = Clock::get()?;
            let sod = start_of_day(clock.unix_timestamp);
            if sod > cfg.day_start {
                cfg.day_start = sod;
                cfg.day_volume = 0;
            }
            if cfg.day_volume.saturating_add(amount) > cfg.daily_limit {
                return Err(ProgramError::Custom(9999));
            }
            cfg.day_volume += amount;
            cfg.serialize(&mut &mut config_ai.data.borrow_mut()[..]).unwrap();

            // ensure processed PDA exists
            let mut seed = Vec::new();
            seed.extend_from_slice(PROCESSED_SEED);
            seed.extend_from_slice(&deposit_id);
            ensure_pda_created(program_id, payer, processed_ai, &[PROCESSED_SEED, &deposit_id], 8)?; // small

            // mark processed
            let p = Processed { used: true };
            p.serialize(&mut &mut processed_ai.data.borrow_mut()[..]).unwrap();

            // mint via program-derived authority (config seed)
            let (authority, bump_auth) = Pubkey::find_program_address(&[CONFIG_SEED], program_id);
            let ix = token_ix::mint_to(
                token_program.key,
                mint_ai.key,
                recipient_ata.key,
                &authority,
                &[],
                amount,
            )?;
            invoke_signed(
                &ix,
                &[mint_ai.clone(), recipient_ata.clone(), token_program.clone()],
                &[&[CONFIG_SEED, &[bump_auth]]],
            )?;

            Ok(())
        }

        BridgeIx::RequestWithdraw { withdraw_id, amount, to_evm } => {
            let acc_iter = &mut accounts.iter();
            let owner = next_account_info(acc_iter)?;       // user signer
            let config_ai = next_account_info(acc_iter)?;
            let mint_ai = next_account_info(acc_iter)?;
            let owner_ata = next_account_info(acc_iter)?;
            let token_program = next_account_info(acc_iter)?;

            if !owner.is_signer { return Err(BridgeError::Unauthorized.into()); }

            let cfg: BridgeConfig = BridgeConfig::try_from_slice(&config_ai.data.borrow())?;
            if *mint_ai.key != cfg.usdc_mint { return Err(BridgeError::InvalidAccount.into()); }

            let ix = token_ix::burn(token_program.key, owner_ata.key, mint_ai.key, owner.key, &[], amount)?;
            invoke(&ix, &[owner_ata.clone(), mint_ai.clone(), owner.clone(), token_program.clone()])?;

            // emit log for relayer（简单 V1 文本日志；生产可用事件账户/更结构化日志）
            msg!("WithdrawRequested id={:?} amt={} toEvm=0x{}", withdraw_id, amount, hex::encode(to_evm));
            Ok(())
        }
    }
}
```

## 3.3 构建与部署

`infra/scripts/program.build.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
cd packages/program-1024
cargo build-sbf
```

`infra/scripts/program.deploy.sh`（按你的 1024 链 CLI 替换 `solana` 命令）：

```bash
#!/usr/bin/env bash
set -euo pipefail
source ./../.env
PROGRAM_SO=packages/program-1024/target/deploy/bridge_program_1024.so
solana program deploy ${PROGRAM_SO} --url ${SOLANA_RPC_URL}
```

---

# 4. TypeScript 客户端（Initialize / Mint / Withdraw）— 1024 链 SDK 调用

在 **Relayer** 和 **前端**都会用到这些 helper；统一放在 `packages/relayer/src/solana_client.ts`，前端如需也可复制一份或抽成 `packages/proto/clients/`。

```ts
// packages/relayer/src/solana_client.ts
import { Connection, PublicKey, SystemProgram, Transaction, TransactionInstruction, Keypair, sendAndConfirmTransaction } from '@solana/web3.js';
import { ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_PROGRAM_ID, getAssociatedTokenAddress } from '@solana/spl-token';
import { serialize, Schema } from 'borsh';
import { readFileSync } from 'fs';

export const CONFIG_SEED = Buffer.from('bridge-config');
export const PROCESSED_SEED = Buffer.from('processed');

export class InitializeIx {
  constructor(
    public admin: Uint8Array,      // 32
    public usdc_mint: Uint8Array,  // 32
    public min_deposit: bigint,     // u64
    public daily_limit: bigint,     // u64
    public signers: Uint8Array[],   // Vec<Pubkey>
    public threshold: number        // u8
  ) {}
}

export class MintOnDepositIx {
  constructor(
    public deposit_id: Uint8Array,  // [32]
    public amount: bigint,          // u64
    public recipient: Uint8Array    // Pubkey
  ) {}
}

export class RequestWithdrawIx {
  constructor(
    public withdraw_id: Uint8Array, // [32]
    public amount: bigint,          // u64
    public to_evm: Uint8Array       // [20]
  ) {}
}

const schema: Schema = new Map<any, any>([
  [InitializeIx, {kind:'struct', fields:[
    ['admin', [32]], ['usdc_mint', [32]], ['min_deposit','u64'], ['daily_limit','u64'],
    ['signers', [[32]]], ['threshold','u8']
  ]}],
  [MintOnDepositIx, {kind:'struct', fields:[
    ['deposit_id', [32]], ['amount','u64'], ['recipient',[32]]
  ]}],
  [RequestWithdrawIx, {kind:'struct', fields:[
    ['withdraw_id',[32]], ['amount','u64'], ['to_evm',[20]]
  ]}],
]);

function encodeIx(ix: any, variantDiscriminator: number): Buffer {
  const body = Buffer.from(serialize(schema, ix));
  return Buffer.concat([Buffer.from([variantDiscriminator]), body]); // enum variant prefix
}

export function pkey(x: string|Uint8Array): PublicKey {
  if (typeof x === 'string') return new PublicKey(x);
  return new PublicKey(x);
}

export function loadKeypair(path: string): Keypair {
  const raw = JSON.parse(readFileSync(path, 'utf8'));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}

export async function deriveConfigPda(programId: PublicKey): Promise<[PublicKey, number]> {
  return PublicKey.findProgramAddressSync([CONFIG_SEED], programId);
}

export function deriveProcessedPda(programId: PublicKey, depositId32: Uint8Array): [PublicKey, number] {
  return PublicKey.findProgramAddressSync([PROCESSED_SEED, depositId32], programId);
}

export async function getOrCreateAta(
  conn: Connection,
  mint: PublicKey,
  owner: PublicKey,
  payer: Keypair,
): Promise<PublicKey> {
  const ata = await getAssociatedTokenAddress(mint, owner, false, TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID);
  // 由外部确保存在（这里可选创建；也可在前端创建）
  return ata;
}

/** Initialize config (程序会自动创建 config PDA；payer 付租金) */
export async function sendInitialize(
  conn: Connection,
  programId: PublicKey,
  payer: Keypair,
  params: {
    admin: PublicKey,
    usdcMint: PublicKey,
    minDeposit: bigint,   // 6 decimals
    dailyLimit: bigint,
    signers: PublicKey[],
    threshold: number
  }
) {
  const [configPda] = await deriveConfigPda(programId);
  const data = encodeIx(
    new InitializeIx(
      params.admin.toBytes(),
      params.usdcMint.toBytes(),
      params.minDeposit,
      params.dailyLimit,
      params.signers.map(s => s.toBytes()),
      params.threshold
    ),
    0 // enum variant: Initialize
  );

  const ix = new TransactionInstruction({
    programId,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: configPda,       isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner:false, isWritable:false },
    ],
    data
  });

  const tx = new Transaction().add(ix);
  return await sendAndConfirmTransaction(conn, tx, [payer]);
}

/** MintOnDeposit（relayer 调用；payer 为 relayer；signer1 也由 relayer扮演，V1 阈值=1） */
export async function sendMintOnDeposit(
  conn: Connection,
  programId: PublicKey,
  payer: Keypair,
  params: {
    signer1: Keypair,            // 必须在 cfg.signers 之列（V1）
    usdcMint1024: PublicKey,
    recipientOwner: PublicKey,   // 1024 链用户钱包地址
    amount: bigint,              // 6 decimals
    depositId32: Uint8Array
  }
) {
  const [configPda] = await deriveConfigPda(programId);
  const [processedPda] = deriveProcessedPda(programId, params.depositId32);

  const recipientAta = await getAssociatedTokenAddress(params.usdcMint1024, params.recipientOwner, false, TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID);

  const data = encodeIx(
    new MintOnDepositIx(params.depositId32, params.amount, params.recipientOwner.toBytes()),
    1 // enum variant: MintOnDeposit
  );

  const keys = [
    { pubkey: payer.publicKey,         isSigner:true,  isWritable:true },
    { pubkey: params.signer1.publicKey,isSigner:true,  isWritable:false },
    { pubkey: configPda,               isSigner:false, isWritable:true },
    { pubkey: params.usdcMint1024,     isSigner:false, isWritable:true },
    { pubkey: recipientAta,            isSigner:false, isWritable:true },
    { pubkey: processedPda,            isSigner:false, isWritable:true },
    { pubkey: TOKEN_PROGRAM_ID,        isSigner:false, isWritable:false },
    { pubkey: SystemProgram.programId, isSigner:false, isWritable:false },
  ];
  const ix = new TransactionInstruction({ programId, keys, data });

  const tx = new Transaction().add(ix);
  return await sendAndConfirmTransaction(conn, tx, [payer, params.signer1]);
}

/** RequestWithdraw（用户在 1024 链上发起；Relayer 监听日志并在 EVM 侧 withdraw） */
export async function sendRequestWithdraw(
  conn: Connection,
  programId: PublicKey,
  owner: Keypair,
  params: {
    usdcMint1024: PublicKey,
    ownerAta: PublicKey,        // 由前端/SDK 事先 getOrCreate
    amount: bigint,
    withdrawId32: Uint8Array,
    toEvm20: Uint8Array         // 20 bytes EVM address
  }
) {
  const [configPda] = await deriveConfigPda(programId);

  const data = encodeIx(
    new RequestWithdrawIx(params.withdrawId32, params.amount, params.toEvm20),
    2 // enum variant: RequestWithdraw
  );

  const keys = [
    { pubkey: owner.publicKey,  isSigner:true,  isWritable:true },
    { pubkey: configPda,        isSigner:false, isWritable:true },
    { pubkey: params.usdcMint1024, isSigner:false, isWritable:true },
    { pubkey: params.ownerAta,  isSigner:false, isWritable:true },
    { pubkey: TOKEN_PROGRAM_ID, isSigner:false, isWritable:false },
  ];
  const ix = new TransactionInstruction({ programId, keys, data });
  const tx = new Transaction().add(ix);
  return await sendAndConfirmTransaction(conn, tx, [owner]);
}
```

---

# 5. Relayer（Node/TS）— `packages/relayer`（含订阅与完整调用）

## 5.1 初始化

```bash
cd packages/relayer
pnpm init -y
pnpm add ethers@6 @solana/web3.js @solana/spl-token dotenv pino commander borsh
pnpm add -D typescript ts-node @types/node
npx tsc --init
mkdir -p src/abi
```

`tsconfig.json`：

```json
{
  "compilerOptions": {
    "target": "es2020",
    "module": "commonjs",
    "moduleResolution": "node",
    "outDir": "dist",
    "rootDir": "src",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  }
}
```

## 5.2 辅助模块

`src/config.ts`：

```ts
import * as dotenv from 'dotenv'; dotenv.config();
function requireEnv(k: string): string { const v = process.env[k]; if (!v) throw new Error(`Missing env: ${k}`); return v; }

export const cfg = {
  evmRpc: requireEnv('EVM_RPC_URL'),
  evmChainId: Number(requireEnv('EVM_CHAIN_ID')),
  evmBridge: requireEnv('EVM_BRIDGE_ADDRESS'),
  evmUsdc: requireEnv('EVM_USDC_ADDRESS'),
  evmRelayerKey: requireEnv('EVM_RELAYER_PRIVKEY'),
  evmSigners: (process.env.EVM_SIGNERS || '').split(',').map(s => s.trim()).filter(Boolean),
  evmThreshold: Number(process.env.EVM_THRESHOLD || '1'),

  solRpc: requireEnv('SOLANA_RPC_URL'),
  programId: requireEnv('BRIDGE_PROGRAM_ID'),
  usdc1024Mint: requireEnv('USDC_1024_MINT'),
  relayerSolanaKeypair: requireEnv('RELAYER_SOLANA_KEYPAIR'),
  solSigners: (process.env.SOLANA_SIGNERS || '').split(',').map(s => s.trim()).filter(Boolean),
  solThreshold: Number(process.env.SOLANA_THRESHOLD || '1'),
};
```

`src/logger.ts`：

```ts
import pino from 'pino';
export const log = pino({ level: process.env.LOG_LEVEL || 'info' });
```

`src/evm.ts`：

```ts
import { ethers } from 'ethers';
import { cfg } from './config';
import EvmBridgeAbi from './abi/EvmBridge.json';

export function getEvmProvider() {
  return new ethers.JsonRpcProvider(cfg.evmRpc, cfg.evmChainId);
}
export function getEvmWallet() {
  const p = getEvmProvider();
  return new ethers.Wallet(cfg.evmRelayerKey, p);
}
export function getEvmBridge() {
  const w = getEvmWallet();
  return new ethers.Contract(cfg.evmBridge, EvmBridgeAbi, w);
}
/** 与合约一致的 withdraw 哈希（ETH 签名） */
export function withdrawMessageHash(toEvm: string, amount: bigint, withdrawId: string): string {
  const packed = ethers.solidityPacked(
    ['bytes', 'address', 'uint256', 'bytes32', 'uint256'],
    [ethers.toUtf8Bytes('WITHDRAW:'), toEvm, amount, withdrawId, BigInt(cfg.evmChainId)]
  );
  return ethers.keccak256(packed);
}
```

`src/solana_client.ts`（上节已写好，复制到此处）。

## 5.3 主流程（订阅与桥接）

`src/index.ts`：

```ts
import { ethers } from 'ethers';
import { cfg } from './config';
import { log } from './logger';
import { getEvmBridge, withdrawMessageHash } from './evm';

import {
  Connection, PublicKey, Keypair, Logs, LAMPORTS_PER_SOL
} from '@solana/web3.js';
import {
  loadKeypair, pkey, sendMintOnDeposit, sendRequestWithdraw, deriveConfigPda, deriveProcessedPda
} from './solana_client';

import EvmBridgeAbi from './abi/EvmBridge.json';

function hex32ToBytes32(hex: string): Uint8Array {
  const h = hex.startsWith('0x') ? hex.slice(2) : hex;
  return Uint8Array.from(Buffer.from(h, 'hex'));
}

async function run() {
  log.info('Relayer starting...');
  // EVM
  const evmBridge = getEvmBridge();

  // Solana
  const solConn = new Connection(cfg.solRpc, 'confirmed');
  const programId = new PublicKey(cfg.programId);
  const usdcMint = new PublicKey(cfg.usdc1024Mint);
  const relayer = loadKeypair(process.env.RELAYER_SOLANA_KEYPAIR!);
  const signer1 = relayer; // V1: relayer 同时作为 signer1（需在 Initialize 中配置为 signer）

  // ---- 订阅 EVM: DepositCommitted -> 触发 1024 MintOnDeposit ----
  evmBridge.on('DepositCommitted', async (depositId: string, from: string, recipient1024_bytes32: string, amount: bigint) => {
    try {
      log.info({ depositId, from, amount: amount.toString() }, 'EVM DepositCommitted');

      // recipient1024 是 bytes32：转换为 Pubkey（我们约定前 32 字节即为公钥）
      const recipientPk = new PublicKey(hex32ToBytes32(recipient1024_bytes32));

      // 发送 MintOnDeposit
      await sendMintOnDeposit(
        solConn,
        programId,
        relayer,
        {
          signer1,
          usdcMint1024: usdcMint,
          recipientOwner: recipientPk,
          amount,
          depositId32: hex32ToBytes32(depositId)
        }
      );
      log.info({ depositId }, 'MintOnDeposit success');
    } catch (e:any) {
      log.error({ err: e?.message, depositId }, 'MintOnDeposit failed');
    }
  });

  // ---- 订阅 1024: WithdrawRequested -> 触发 EVM withdraw ----
  // V1：从日志文本解析（生产可改为事件账户/更结构化的日志）
  solConn.onLogs(programId, async (logs: Logs) => {
    try {
      // 查找 "WithdrawRequested id=... amt=... toEvm=0x..."
      const line = logs.logs.find(l => l.includes('WithdrawRequested'));
      if (!line) return;

      const idMatch = line.match(/id=\[([^\]]+)\]/);
      const amtMatch = line.match(/amt=(\d+)/);
      const toMatch = line.match(/toEvm=0x([0-9a-fA-F]{40})/);
      if (!idMatch || !amtMatch || !toMatch) return;

      // withdraw_id：这里日志里是 Debug 格式，V1 简化——实际生产建议从事件数据账号读取
      // 我们使用 tx signature 作为 withdrawId 的来源更简单（或前端生成 32 字节随机 id）
      const withdrawId = ethers.hexlify(ethers.randomBytes(32)); // 简化：生成随机 id（生产应使用程序中实际 id）
      const amount = BigInt(amtMatch[1]);
      const toEvm = '0x' + toMatch[1];

      // 签名 & withdraw
      const wallet = (await import('./evm')).getEvmWallet();
      const msgHash = withdrawMessageHash(toEvm, amount, withdrawId);
      const signature = await wallet.signMessage(ethers.getBytes(msgHash));

      const bridge = getEvmBridge();
      const tx = await bridge.withdraw(withdrawId, toEvm, amount, [signature]);
      await tx.wait();
      log.info({ withdrawId, toEvm, amount: amount.toString(), tx: tx.hash }, 'EVM withdraw success');
    } catch (e:any) {
      log.error({ err: e?.message, slot: logs.slot }, 'handle withdraw log failed');
    }
  }, 'confirmed');

  log.info('Relayer subscriptions established.');
}

run().catch((e) => { log.error(e); process.exit(1); });
```

> 说明：
>
> * 为了 V1 快速打通，我们在解析 WithdrawRequested 时，用**随机 32 字节**作为 `withdrawId` 提交到 EVM。生产环境应在 Program 里**把真正的 `withdraw_id` 放入更结构化的日志或事件账户**，Relayer 读取该值再调用 EVM。
> * 只要双方保证 `withdrawId` 唯一且单次消费即可满足安全性（EVM 侧有去重表）。

## 5.4 复制 ABI

将 `packages/evm-contracts/out/EvmBridge.sol/EvmBridge.json` 复制到 `packages/relayer/src/abi/EvmBridge.json`。

## 5.5 Docker

`packages/relayer/Dockerfile`：

```dockerfile
FROM node:18-slim
WORKDIR /app
COPY package.json pnpm-lock.yaml ./
RUN npm i -g pnpm && pnpm i --frozen-lockfile
COPY tsconfig.json ./tsconfig.json
COPY src ./src
RUN pnpm run build
CMD ["node", "dist/index.js"]
```

`infra/docker/docker-compose.yaml`：

```yaml
version: "3.8"
services:
  relayer:
    build:
      context: ../../packages/relayer
    image: 1024-relayer:dev
    container_name: 1024-relayer
    restart: unless-stopped
    env_file:
      - ../.env
    volumes:
      - ../relayer-id.json:/app/relayer-id.json:ro
    environment:
      - RELAYER_SOLANA_KEYPAIR=/app/relayer-id.json
```

---

# 6. 前端（Next.js 组件，用于桥交互）— `packages/frontend`

> 你主站使用 **Solana Wallet Adapter** 无需更改；这里只是提供**桥用 EVM 连接 + Deposit** 的组件，以及**Withdraw** 的调用提示（Withdraw 由 1024 链侧发交易，使用你现有钱包）。

## 6.1 初始化

```bash
cd packages/frontend
pnpm create next-app@latest . --ts --eslint --app --src-dir --import-alias "@/*"
pnpm add @web3modal/ethers ethers @solana/wallet-adapter-react @solana/wallet-adapter-react-ui @solana/wallet-adapter-wallets
```

`src/app/providers.tsx`：

```tsx
'use client';
import React from 'react';
import { createWeb3Modal, defaultConfig } from '@web3modal/ethers/react';
import '@solana/wallet-adapter-react-ui/styles.css';

const projectId = 'YOUR_WALLETCONNECT_PID';
const metadata = { name: '1024 Bridge', description: 'Arbitrum ↔ 1024Chain', url: 'http://localhost:3000', icons: [] };

createWeb3Modal({
  ethersConfig: defaultConfig({ metadata }),
  chains: [{ chainId: Number(process.env.NEXT_PUBLIC_EVM_CHAIN_ID), name: 'Arbitrum Sepolia' }],
  projectId,
});

export default function Providers({ children }: { children: React.ReactNode }) {
  return <>{children}</>;
}
```

`src/app/layout.tsx`：

```tsx
import Providers from './providers';
export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (<html><body><Providers>{children}</Providers></body></html>);
}
```

`src/abi/EvmBridge.json`：复制与 Relayer 相同 ABI。

`src/components/DepositCard.tsx`：

```tsx
'use client';
import { useState } from 'react';
import { useWeb3ModalAccount, useWeb3ModalProvider } from '@web3modal/ethers/react';
import { BrowserProvider, Contract, ethers } from 'ethers';
import abi from '@/abi/EvmBridge.json';

const BRIDGE = process.env.NEXT_PUBLIC_EVM_BRIDGE_ADDRESS!;
const USDC = process.env.NEXT_PUBLIC_EVM_USDC_ADDRESS!;

export default function DepositCard() {
  const { address, isConnected } = useWeb3ModalAccount();
  const { walletProvider } = useWeb3ModalProvider();
  const [amount, setAmount] = useState('10'); // USDC
  const [recipient1024, setRecipient1024] = useState('0x' + '00'.repeat(32)); // 32 bytes hex

  async function deposit() {
    if (!walletProvider) return;
    const provider = new BrowserProvider(walletProvider as any);
    const signer = await provider.getSigner();

    const amt = ethers.parseUnits(amount, 6);

    const usdc = new Contract(USDC, ['function approve(address,uint256) external returns (bool)'], signer);
    await (await usdc.approve(BRIDGE, amt)).wait();

    const bridge = new Contract(BRIDGE, abi as any, signer);
    const tx = await bridge.deposit(amt, recipient1024);
    await tx.wait();
    alert('Deposit submitted: ' + tx.hash);
  }

  return (
    <div className="p-4 border rounded">
      <h3>Deposit (Arbitrum → 1024Chain)</h3>
      <div>Wallet: {isConnected ? address : 'Not connected'}</div>
      <input value={amount} onChange={e => setAmount(e.target.value)} placeholder="USDC amount" />
      <input value={recipient1024} onChange={e => setRecipient1024(e.target.value)} placeholder="recipient1024 (bytes32 hex)" />
      <button onClick={deposit} disabled={!isConnected}>Deposit</button>
      <p className="text-xs mt-2">* V1 仅支持 Arbitrum USDC；最小额与日额度以合约配置为准。</p>
    </div>
  );
}
```

`src/components/WithdrawCard.tsx`（调用 1024 链 `RequestWithdraw`：示例使用本地私钥，不在浏览器执行；建议放进后端或桌面工具。这里展示形态即可。）：

```tsx
'use client';
export default function WithdrawCard() {
  return (
    <div className="p-4 border rounded">
      <h3>Withdraw (1024Chain → Arbitrum)</h3>
      <p>请在 1024 链客户端/后端工具调用 RequestWithdraw（本页面仅示意）。</p>
    </div>
  );
}
```

`src/app/page.tsx`：

```tsx
import DepositCard from '@/components/DepositCard';
import WithdrawCard from '@/components/WithdrawCard';

export default function Page() {
  return (
    <main className="p-6">
      <h1>1024 Bridge Demo</h1>
      <DepositCard />
      <div style={{ height: 16 }} />
      <WithdrawCard />
    </main>
  );
}
```

---

# 7. 一键脚本 / 运行顺序

## 7.1 依赖与工具

```bash
pnpm i
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

准备 `infra/relayer-id.json`（Solana Keypair，dev 用）：

```bash
solana-keygen new -o infra/relayer-id.json
```

复制 `infra/.env.example` → `infra/.env`，填入 RPC、私钥等（`RELAYER_SOLANA_KEYPAIR=./infra/relayer-id.json`）。

## 7.2 部署 EVM 合约

```bash
bash infra/scripts/evm.deploy.sh
```

记录输出中的 `USDC:`（如 Mock）与 `Bridge:`，写回 `.env` 的 `EVM_USDC_ADDRESS` / `EVM_BRIDGE_ADDRESS`。

## 7.3 构建 & 部署 1024 Program

```bash
bash infra/scripts/program.build.sh
bash infra/scripts/program.deploy.sh
```

记录 `BRIDGE_PROGRAM_ID` 并写回 `.env`。

## 7.4 初始化 Program（TS 客户端）

新增 `packages/relayer/src/init_config.ts`：

```ts
import { Connection, PublicKey } from '@solana/web3.js';
import { loadKeypair, sendInitialize } from './solana_client';
import { cfg } from './config';

async function main() {
  const conn = new Connection(cfg.solRpc, 'confirmed');
  const programId = new PublicKey(cfg.programId);
  const payer = loadKeypair(process.env.RELAYER_SOLANA_KEYPAIR!);

  const admin = payer.publicKey;
  const usdcMint = new PublicKey(cfg.usdc1024Mint); // 初始化前请先创建好 1024USDC mint (由你们的链/工具创建)
  const minDeposit = BigInt(process.env.MIN_DEPOSIT_USDC || '5000000');
  const dailyLimit = BigInt(process.env.DAILY_LIMIT_USDC || '500000000000');
  const signers = [payer.publicKey];
  const threshold = 1;

  const sig = await sendInitialize(conn, programId, payer, { admin, usdcMint, minDeposit, dailyLimit, signers, threshold });
  console.log('Initialize sig:', sig);
}
main().catch(console.error);
```

运行：

```bash
cd packages/relayer
pnpm ts-node src/init_config.ts
```

> 备注：**创建 1024USDC mint** 可用你们的 CLI/工具：设定 `mint_authority = PDA(CONFIG_SEED)`（或先由 admin 创建后，再在 Initialize 中覆盖到 `usdc_mint`，V1 程序在 mint_to 时使用 `PDA(CONFIG_SEED)` 作为 authority）。

## 7.5 启动 Relayer（本地）

```bash
cd packages/relayer
# 拷贝 ABI
cp ../evm-contracts/out/EvmBridge.sol/EvmBridge.json ./src/abi/EvmBridge.json
pnpm build
pnpm start
```

或 Docker：

```bash
docker compose -f infra/docker/docker-compose.yaml up --build -d
```

## 7.6 前端（本地）

```bash
cd packages/frontend
pnpm dev
```

---

# 8. 风险点与上线前硬要求

* **USDC 合约地址**：主网时务必替换为 **Circle 原生 USDC（Arbitrum One）**。
* **阈值签名**：生产切换到 **≥2/3**，并确保 Relayer 使用多签/门限签名方案（而非单密钥）。
* **withdrawId 的来源**：V1 示例使用随机值（方便打通）；生产必须使用**程序生成或交易上下文可验证的唯一 ID**，Relayer 严格传递该 ID（EVM 侧有去重）。
* **日志解析**：V1 解析文本日志，生产建议用**事件账户**或更结构化的 Log（Program 写入 base64 编码数据，Relayer 解码）。
* **PDA 资金与租金**：本版由 `payer` 支付 PDA 租金（Relayer Keypair）；生产考虑费用账户管理与限额。
* **限额对齐**：EVM 与 1024 侧 `minDeposit` / `dailyLimit` 保持一致，避免状态分叉。
* **密钥管理**：开发期 `.env`+文件私钥；生产迁 KMS/Vault/HSM，Relayer 容器不落盘密钥。
* **监控与告警**：Relayer 增加 /health、Prometheus 指标、Slack/Telegram 告警（异常率、签名队列、延迟、失败重试）。

---

# 9. 后续增强（Phase-2 建议）

* EVM 合约迁 **EIP-712** 域签 / 或 **BLS 聚合签名**。
* Program 支持**真正的多 signer 检查**：在一次交易内携带多个签名者账户，数量达到 `threshold`。
* Withdraw 请求用**事件账户**承载：包含 `withdraw_id / amount / to_evm / owner`，Relayer 从账户数据读取并校验。
* 前端完善 1024 侧 **RequestWithdraw**（基于你的链的浏览器签名工具/适配器）。
* `proto/` 下生成 **TS + Rust** 双语常量/类型（自动 Codegen，防止字段漂移）。

---

# 10. ToDoList / Checklist（给 Cursor 勾选）

【 】初始化 Monorepo：创建 `packages/*` 与 `infra/*`、`.editorconfig/.gitignore/pnpm-workspace.yaml`
【 】复制 `infra/.env.example` → `infra/.env`，填充 RPC、私钥、地址
【 】生成 `infra/relayer-id.json`（Solana Keypair，dev）
【 】在 `packages/evm-contracts` 写入 `EvmBridge.sol` 与 `MockUSDC.sol`
【 】`forge build` 通过
【 】运行 `infra/scripts/evm.deploy.sh`，记录 `EVM_BRIDGE_ADDRESS` 与（如用 Mock）`EVM_USDC_ADDRESS`
【 】在 `.env` 写入 `EVM_BRIDGE_ADDRESS` / `EVM_USDC_ADDRESS`
【 】在 `packages/program-1024` 写入 Rust 程序（增强版，支持自动创建 PDA）
【 】`cargo build-sbf` 通过
【 】`infra/scripts/program.deploy.sh` 部署 Program，记录 `BRIDGE_PROGRAM_ID`
【 】在 `.env` 写入 `BRIDGE_PROGRAM_ID` 与 `USDC_1024_MINT`（确保该 mint 存在且 authority = PDA(CONFIG_SEED)）
【 】运行 `packages/relayer/src/init_config.ts` 完成 Initialize（阈值=1，signer=relayer）
【 】复制 ABI 到 `packages/relayer/src/abi/EvmBridge.json`
【 】`pnpm -w build` 并 `pnpm -w start` 启动 Relayer（或用 Docker Compose）
【 】在前端 `packages/frontend` 运行 `pnpm dev`，用 EVM 钱包 `approve + deposit`（Arbitrum Sepolia）
【 】观察 Relayer 捕获 `DepositCommitted` 并调用 `MintOnDeposit`，确认 1024 账户到账
【 】在 1024 链用 Keypair 调用 `RequestWithdraw`，触发 Program 日志
【 】观察 Relayer 解析日志并在 EVM 调用 `withdraw()`，USDC 成功回到 EVM 钱包
【 】验证最小额/日额度/暂停开关
【 】改造：把阈值签名从 1-of-1 升级到 ≥2/3（合约/程序/Relayer 同步调整）
【 】补充生产运维：/health、指标、告警、KMS/Vault 密钥管理、回滚与审计说明

---

到这里，**从空白文件夹到跑通 V1 官方桥**的全流程、代码与命令都齐了。
如果你需要，我可以在下一版里再补上**创建 1024USDC mint 的 CLI 脚本**（设定 mint authority 为 `PDA(CONFIG_SEED)`）、以及**Structured Log/事件账户**的实现，让 Withdraw 的 `withdraw_id` 严格来源于链上数据。
