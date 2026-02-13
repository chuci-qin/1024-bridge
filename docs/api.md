# API 文档

## 目录

- [API 概览](#api-概览)
- [智能合约 API](#智能合约-api)
  - [发送端合约 API](#发送端合约-api)
  - [接收端合约 API](#接收端合约-api)
- [Gateway 服务 API](#gateway-服务-api)
- [Relayer 服务 API](#relayer-服务-api)
- [模块间调用规约](#模块间调用规约)
- [数据结构](#数据结构)
- [配置参数](#配置参数)

---

## API 概览

### 智能合约 API 总览表

| 模块 | 类别 | 接口名称 | 权限 | 主要参数 | 返回值/输出 | 功能效果 |
|------|------|----------|------|----------|-------------|----------|
| 统一合约 | 初始化 | `initialize` | `onlyAdmin` | `adminAddress`(多签) | 无 | 统一初始化发送端和接收端合约。EVM端：合约本身作为金库；SVM端：使用PDA金库 |
| 统一合约 | 配置 | `configure_usdc` | `onlyAdmin` | `usdcAddress` | 无 | 配置USDC代币地址（SVM为mint account，EVM为合约地址） |
| 统一合约 | 配置 | `configure_peer` | `onlyAdmin` | `peerContract`, `sourceChainId`, `targetChainId` | 无 | 统一配置对端合约和链ID（同时配置发送端和接收端） |
| 发送端合约 | 质押 | `stake` | 公开 | `amount`, `receiverAddress` | `nonce` | 质押USDC，触发`StakeEvent`事件，nonce自动递增 |
| 接收端合约 | 白名单管理 | `addRelayer` | `onlyAdmin` | `relayerAddress` | 无 | 添加Relayer到白名单 |
| 接收端合约 | 白名单管理 | `removeRelayer` | `onlyAdmin` | `relayerAddress` | 无 | 从白名单移除Relayer |
| 接收端合约 | 白名单查询 | `isRelayer` | 公开（view） | `relayerAddress` | `bool` | 查询地址是否为Relayer |
| 接收端合约 | 白名单查询 | `getRelayerCount` | 公开（view） | 无 | `uint256` | 查询当前Relayer数量 |
| 接收端合约 | 签名验证 | `submitSignature` | `onlyWhitelistedRelayer` | `eventData`, `signature` | 无 | 提交签名，达到阈值后解锁代币 |
| 接收端合约 | 流动性管理 | `addLiquidity` | `onlyAdmin` | `amount` | 无 | 从多签钱包向PDA金库增加流动性 |
| 接收端合约 | 流动性管理 | `withdrawLiquidity` | `onlyAdmin` | `amount` | 无 | 从PDA金库向多签钱包提取流动性 |

### Gateway 服务 API 总览表

| 模块 | 类别 | 功能名称 | 权限 | 主要参数 | 输出 | 功能效果 |
|------|------|----------|------|----------|------|----------|
| EVM Gateway服务 | HTTP API | `POST /stake` | 公开 | `amount`, `target_address` | `success`, `message`, `tx_hash` | 接收跨链请求，调用 EVM stake 接口完成从 Arbitrum 到 1024chain 的跨链（Deposit 方向第二步） |
| Withdraw Gateway服务 | HTTP API | `POST /withdraw` | 公开 | `target_chain`, `target_asset`, `usdc_amount`, `recipient_address` | `success`, `message`, `route_id`, `tx_hash` | 接收提现请求，使用 LiFi SDK 完成从 Arbitrum 到任意链的跨链（Withdraw 方向第二步） |

### Relayer 服务 API 总览表

| 模块 | 类别 | 功能名称 | 权限 | 主要参数 | 输出 | 功能效果 |
|------|------|----------|------|----------|------|----------|
| Relayer服务 | 事件监听 | 监听`StakeEvent` | 无 | 无 | 事件数据 | 监听发送端链的质押事件 |
| Relayer服务 | 签名转发 | `processEvent` | 内部 | `eventData` | 交易哈希 | 验证事件、生成签名并提交到接收端合约 |

### 事件总览表

| 模块 | 事件名称 | 触发条件 | 事件参数 | 用途 |
|------|----------|----------|----------|------|
| 发送端合约 | `StakeEvent` | 用户调用`stake` | `sourceContract`, `targetContract`, `chainId`, `blockHeight`, `amount`, `receiverAddress`, `nonce` | 通知Relayer处理跨链转账 |

### 数据结构总览表

| 数据结构 | 用途 | 字段 | 说明 |
|----------|------|------|------|
| `StakeEventData` | 质押事件数据 | `sourceContract`, `targetContract`, `chainId`, `blockHeight`, `amount`, `receiverAddress`, `nonce` | 跨链转账的完整事件数据 |

### 配置参数总览表

| 配置项 | 测试网 | 主网 | 说明 |
|--------|--------|------|------|
| Arbitrum RPC | `https://sepolia-rollup.arbitrum.io/rpc` | `https://arb1.arbitrum.io/rpc` | Arbitrum网络RPC端点 |
| 1024chain RPC | `https://rpc-testnet.1024chain.com/rpc/` | 待配置 | 1024chain网络RPC端点 |
| Arbitrum Chain ID | 421614 | 42161 | 链标识符 |
| 1024chain Chain ID | 91024 | 待确认 | 链标识符 |
| Relayer数量 | ≥ 3，最多18个 | ≥ 3，最多18个 | 中继节点数量 |
| 签名阈值 | > 2/3 * relayer总数 | > 2/3 * relayer总数 | 解锁所需签名比例 |
| 未完成请求 | 至少100个 | 至少100个 | 同时支持的未完成跨链请求数量 |
| 签名缓存 | 至少1200个 | 至少1200个 | 同时缓存的relayer签名数量（100请求×18relayer） |

---

## 智能合约 API

### 统一初始化 API（SVM）

**重要变更：** 在 Solana (SVM) 平台上，发送端和接收端合约的初始化合并为一个 `initialize` 指令。

#### 统一初始化

```
function initialize(
    address vaultAddress,      // 质押金库地址（发送端和接收端共享）
    address adminAddress       // 管理员钱包地址（发送端和接收端共享）
) onlyAdmin
```

**参数说明：**
- `vaultAddress`：PDA 金库地址（发送端和接收端共享同一个金库），由程序控制，支持自动转账
- `adminAddress`：多签钱包地址（发送端和接收端共享同一个管理员），用于管理操作

**权限：** 仅管理员可调用

**功能描述：**
1. 同时创建 `SenderState` 和 `ReceiverState` 账户
2. 创建 PDA 金库（vault）和对应的 token account
3. 初始化发送端 nonce 为 0
4. 初始化接收端 last_nonce 为 0
5. 设置共享的 vault（PDA）和 admin（多签钱包）地址

**注意事项：**
- `vaultAddress` 必须是 PDA 地址，种子为 `[b"vault"]`
- `adminAddress` 可以是多签钱包地址，合约只验证签名
- 多签投票在外部（Squad 程序）处理，合约不关心多签逻辑

#### 配置USDC代币地址

```
function configure_usdc(
    address usdcAddress        // USDC代币地址
) onlyAdmin
```

**参数说明：**
- `usdcAddress`：USDC代币地址
  - **SVM端**：USDC的SPL Token mint account地址（Pubkey）
  - **EVM端**：USDC的ERC20合约地址（address）

**权限：** 仅管理员可调用

**功能描述：**
1. 同时配置发送端和接收端的 `usdc_mint` 字段
2. 因为两端使用同一个USDC代币，所以共享相同的地址
3. 必须在调用 `stake` 或 `submit_signature` 之前配置

**注意事项：**
- 不同网络（测试网/主网）的USDC地址不同，需要根据部署网络正确配置
- 配置后，所有质押和解锁操作都将使用该USDC地址
- **必须在调用 `stake` 或 `submit_signature` 之前配置，否则这些函数会返回错误**

**错误处理：**
- 如果未配置USDC地址，`stake` 和 `submit_signature` 函数会返回错误 "USDC address not configured"
- 可以通过检查 `usdc_mint` 是否为无效地址（如 `Pubkey::default()` 或 `address(0)`）来判断是否已配置

### 统一初始化 API（EVM）

**重要变更：** 在 EVM 平台上，发送端和接收端合约的初始化合并为一个 `initialize` 指令。

#### 统一初始化

```
function initialize(
    address adminAddress       // 管理员钱包地址（发送端和接收端共享）
) onlyAdmin
```

**参数说明：**
- `adminAddress`：具有管理权限的钱包地址（发送端和接收端共享同一个管理员）
  - **支持多签钱包**：可以是 Gnosis Safe 等多签钱包地址
  - **权限检查**：合约只验证 `msg.sender == adminAddress`，多签逻辑在外部处理

**权限：** 仅管理员可调用（初始化时由部署者调用）

**功能描述：**
1. 同时创建 `SenderState` 和 `ReceiverState` 状态
2. 将 vault 设置为 `address(this)`（合约本身）
3. 初始化发送端 nonce 为 0
4. 初始化接收端 lastNonce 为 0
5. 设置 admin 地址

**金库设计：**
- ✅ **合约即金库**：`senderState.vault` 和 `receiverState.vault` 都指向 `address(this)`
- ✅ **无需 approve**：解锁时使用 `transfer()` 而非 `transferFrom()`
- ✅ **简化部署**：不需要单独的 vault 地址配置
- ✅ **简化流动性管理**：直接向合约地址转入 USDC 即可
- **使用示例**：
  ```solidity
  // 初始化
  initialize(
      adminAddress
      adminAddress    // 实际使用的 admin 地址
  );
  
  // 质押时，代币转入合约本身
  IERC20(usdc).transferFrom(msg.sender, address(this), amount);
  
  // 解锁时，合约直接转账（无需 approve）
  IERC20(usdc).transfer(receiver, amount);
  ```

**优势：**
- **简化架构**：减少了外部依赖和配置复杂度
- **安全性提升**：减少了攻击面，不需要管理 approve 权限
- **Gas 优化**：`transfer()` 比 `transferFrom()` 更便宜
- **易于理解**：合约即金库，概念更清晰

#### 配置USDC代币地址（EVM）

```
function configure_usdc(
    address usdcAddress        // USDC ERC20合约地址
) onlyAdmin
```

**参数说明：**
- `usdcAddress`：USDC ERC20合约地址

**权限：** 仅管理员可调用（`msg.sender == admin`，admin 可以是多签钱包）

**功能描述：**
1. 同时配置发送端和接收端的 `usdcContract` 字段
2. 必须在调用 `stake` 或 `submit_signature` 之前配置

#### 统一对端配置（EVM）

```
function configure_peer(
    address peerContract,      // 对端合约地址（发送端和接收端共享同一个对端）
    uint256 sourceChainId,     // 自己的 chain id
    uint256 targetChainId      // 对端的 chain id
) onlyAdmin
```

**参数说明：**
- `peerContract`：对端合约地址（发送端和接收端共享同一个对端）
- `sourceChainId`：当前链的 chain id
- `targetChainId`：对端链的 chain id

**权限：** 仅管理员可调用（`msg.sender == admin`，admin 可以是多签钱包）

**功能描述：**
1. 同时配置发送端的 `targetContract`、`sourceChainId`、`targetChainId`
2. 同时配置接收端的 `sourceContract`、`sourceChainId`、`targetChainId`
3. 因为对端是同一个，所以两个配置共享相同的参数

#### 统一对端配置

```
function configure_peer(
    address peerContract,      // 对端合约地址（发送端和接收端共享同一个对端）
    uint256 sourceChainId,     // 自己的 chain id
    uint256 targetChainId      // 对端的 chain id
) onlyAdmin
```

**参数说明：**
- `peerContract`：对端合约地址（发送端和接收端共享同一个对端，所以只需要一个地址）
- `sourceChainId`：当前链的 chain id
- `targetChainId`：对端链的 chain id

**权限：** 仅管理员可调用

**功能描述：**
1. 同时配置发送端的 `target_contract`、`source_chain_id`、`target_chain_id`
2. 同时配置接收端的 `source_contract`、`source_chain_id`、`target_chain_id`
3. 因为对端是同一个，所以两个配置共享相同的参数

### 发送端合约 API

#### 质押接口

```
function stake(
    uint256 amount,            // 质押数量
    string memory receiverAddress  // 接收端地址
) returns (uint256 nonce)
```

**参数说明：**
- `amount`：质押的 USDC 数量
- `receiverAddress`：接收端链上的接收地址

**返回值：**
- `nonce`：本次质押的唯一序号

**功能描述：**
1. **验证USDC地址已配置**：如果 `usdc_mint` 未配置（为无效地址），返回错误 "USDC address not configured"
2. 将用户的 USDC 转入质押金库地址（使用配置的 `usdc_mint` 地址）
3. 生成单调递增的 nonce（64位无符号整数）：
   - 当前 nonce = `sender_state.nonce`
   - 新 nonce = `current_nonce + 1`
   - 如果 `new_nonce == 0`（溢出），重置为 0
   - 更新 `sender_state.nonce = new_nonce`
4. 触发质押事件

**错误情况：**
- `USDC address not configured`：USDC地址未配置，需要先调用 `configure_usdc` 函数

#### 质押事件

```
event StakeEvent(
    address indexed sourceContract,    // 发送端合约地址
    address indexed targetContract,    // 接收端合约地址
    uint256 chainId,                   // chain id
    uint256 blockHeight,               // 区块高度
    uint256 amount,                    // 质押数量
    string receiverAddress,            // 接收地址
    uint256 nonce                      // 防重放序号
)
```

**字段说明：**
- `sourceContract`：发送端合约地址（防止伪造事件）
- `targetContract`：接收端合约地址（防止伪造事件）
- `chainId`：链 ID
- `blockHeight`：交易发生时的区块高度（防止重放）
- `amount`：质押的代币数量
- `receiverAddress`：接收端的地址
- `nonce`：单调递增的序号（64位无符号整数），防止重放攻击。当达到最大值时自动重置为0

---

### 接收端合约 API

**注意：** 在 SVM 平台上：
- 接收端合约的初始化已合并到统一的 `initialize` 指令中（见上方"统一初始化 API"）
- 接收端合约的对端配置已合并到统一的 `configure_peer` 指令中（见上方"统一对端配置"）

在 EVM 平台上，接收端合约仍使用独立的 `initialize` 和 `configureSource` 函数。

#### Relayer 白名单管理

##### 添加 Relayer

```
function addRelayer(address relayerAddress) onlyAdmin
```

**参数说明：**
- `relayerAddress`：要添加的 relayer 公钥地址

**权限：** 仅管理员可调用

##### 移除 Relayer

```
function removeRelayer(address relayerAddress) onlyAdmin
```

**参数说明：**
- `relayerAddress`：要移除的 relayer 公钥地址

**权限：** 仅管理员可调用

##### 查询 Relayer

```
function isRelayer(address relayerAddress) view returns (bool)
```

**参数说明：**
- `relayerAddress`：要查询的地址

**返回值：**
- `bool`：该地址是否在白名单中

```
function getRelayerCount() view returns (uint256)
```

**返回值：**
- `uint256`：当前白名单中的 relayer 数量

#### 接收 Relayer 消息

```
function submitSignature(
    StakeEventData memory eventData,  // 质押事件数据
    bytes memory signature            // relayer 对事件 hash 的签名
) onlyWhitelistedRelayer
```

**参数说明：**

`StakeEventData` 结构：
```
struct StakeEventData {
    address sourceContract;
    address targetContract;
    uint256 chainId;
    uint256 blockHeight;
    uint256 amount;
    string receiverAddress;
    uint256 nonce;
}
```

- `eventData`：质押事件的完整数据
- `signature`：relayer 使用私钥对事件数据 hash 的签名

**权限：** 仅白名单中的 relayer 可调用

**功能描述：**
1. 验证调用者在 relayer 白名单中
2. **验证USDC地址已配置**：如果 `usdc_mint` 未配置（为无效地址），返回错误 "USDC address not configured"
3. **验证源链合约地址正确**（与配置的 sourceContract 匹配）
4. **验证 chain id 正确**（与配置的 sourceChainId 匹配）
5. **检查 nonce 是否递增**：
   - 如果 `nonce <= last_nonce`，拒绝（重放攻击）
   - 如果 `nonce > last_nonce`，继续处理
6. 获取或创建 `CrossChainRequest` PDA 账户（为每个请求创建独立账户）
7. **初始化或验证 event_data 一致性**（关键安全机制）：
   - **如果是第一个签名**（signatureCount == 0）：
     * 将传入的 `eventData` 存储为"标准答案"
     * 第一个 relayer 提交的 `eventData` 将决定本次跨链请求的所有参数
   - **如果不是第一个签名**（signatureCount > 0）：
     * 验证传入的 `eventData` 是否与已存储的 `eventData` 完全一致
     * 检查所有字段：sourceContract, targetContract, sourceChainId, targetChainId, 
                     blockHeight, amount, receiverAddress, nonce
     * 如果任何字段不匹配，拒绝并返回错误 "Invalid event data: event data must match the first submitted event data"
     * **目的**：防止恶意 relayer 提交不同的 eventData，确保所有 relayer 对相同的事件数据签名
8. 检查该 relayer 是否已为此 nonce 签名
9. **验证签名的合法性**：
   - 验证签名是否匹配传入的 `eventData`
   - SVM 端使用 Ed25519Program 进行密码学验证
   - EVM 端使用 ecrecover 进行 ECDSA 验证
10. 记录该 relayer 的签名到 `CrossChainRequest.signed_relayers`
11. 如果合法签名数量达到阈值（> 2/3 白名单大小），则执行解锁操作
12. **解锁操作**：
    - 从金库向接收地址转账等量 USDC（使用配置的 `usdc_mint` 地址）
    - **重要**：使用存储的 `eventData`（第一个 relayer 提交的），而不是函数参数的 `eventData`
    - 这确保即使函数参数被修改，解锁操作仍使用最初存储的正确数据
13. 更新 `last_nonce = 存储的 eventData.nonce`（标记为已使用），防止重放

**安全性说明：**

系统采用"第一个提交者决定"的设计原则：
- 第一个 relayer 提交的 `eventData` 成为本次跨链请求的"标准答案"
- 后续所有 relayer 必须提交完全相同的 `eventData` 才能通过验证
- 这防止了恶意 relayer 通过提交不同的 `eventData` 来操纵解锁参数（如 amount 或 receiver）
- 解锁时始终使用存储的 `eventData`，确保参数一致性

**错误情况：**
- `USDC address not configured`：USDC地址未配置，需要先调用 `configure_usdc` 函数
- `Invalid event data: event data must match the first submitted event data`：后续 relayer 提交的 eventData 与第一个 relayer 提交的不一致，拒绝签名

#### 流动性管理

##### 增加流动性

```
function addLiquidity(uint256 amount) onlyAdmin
```

**参数说明：**
- `amount`：要增加的 USDC 数量

**权限：** 仅管理员（多签钱包）可调用

**功能描述：**
1. 从多签钱包（admin）的 token account 转账到 PDA 金库的 token account
2. 增加金库的流动性，用于支持跨链解锁操作
3. 需要多签钱包签名（外部处理多签逻辑）

**注意事项：**
- 多签钱包需要先有足够的 USDC 余额
- 多签钱包需要创建对应的 token account
- 合约层面只验证 admin 签名，不关心多签逻辑

##### 提取流动性

```
function withdrawLiquidity(uint256 amount) onlyAdmin
```

**参数说明：**
- `amount`：要提取的 USDC 数量

**权限：** 仅管理员（多签钱包）可调用

**功能描述：**
1. 从 PDA 金库的 token account 转账到多签钱包（admin）的 token account
2. 使用 PDA 作为 authority，无需外部签名
3. 提取金库的流动性，用于资金管理
4. 需要多签钱包签名（外部处理多签逻辑）

**注意事项：**
- 确保金库有足够的余额
- 提取后可能影响跨链解锁能力
- 合约层面只验证 admin 签名，不关心多签逻辑

**使用示例：**

```typescript
// 1. 创建 Squad 多签账户
const squad = new Squad(provider);
const multisig = await squad.createMultisig({
  threshold: 2,
  members: [admin1, admin2, admin3],
});

// 2. 初始化合约（使用多签钱包作为 admin）
await program.methods
  .initialize()
  .accounts({
    admin: multisig.publicKey,  // 多签钱包地址
    vault: vaultPda,  // PDA 金库地址
    // ...
  })
  .rpc();  // 需要多签成员签名（外部处理）

// 3. 增加流动性（从多签钱包到 PDA 金库）
await program.methods
  .addLiquidity(amount)
  .accounts({
    admin: multisig.publicKey,
    adminTokenAccount: multisigTokenAccount,
    vaultTokenAccount: vaultTokenAccountPda,
    // ...
  })
  .rpc();  // 需要多签成员签名（外部处理）

// 4. 提取流动性（从 PDA 金库到多签钱包）
await program.methods
  .withdrawLiquidity(amount)
  .accounts({
    admin: multisig.publicKey,
    adminTokenAccount: multisigTokenAccount,
    vaultTokenAccount: vaultTokenAccountPda,
    // ...
  })
  .rpc();  // 需要多签成员签名（外部处理）

// 5. 解锁操作（使用 PDA，自动执行）
// 当达到阈值后，合约自动从 PDA 金库转账
// 无需多签投票，快速执行
```

**Squad 多签程序信息：**
- 程序地址：`SMPLecH534NA9acB4bMolv7X6RBpK4rjn3LkN1gZXYjy`
- 主要功能：创建多签账户、提案和投票、执行提案
- 多签投票在外部处理，合约不关心多签逻辑

---

## Gateway 服务 API

### EVM Gateway Service API

EVM Gateway Service 提供 HTTP API，用于完成从 Arbitrum 到 1024chain 的跨链转账。

#### API 总览表

| 端点 | 方法 | 功能 | 请求参数 | 响应 | 说明 |
|------|------|------|----------|------|------|
| `/stake` | POST | 调用 EVM stake 合约接口 | `amount`, `target_address` | `success`, `message`, `tx_hash` | 完成从 Arbitrum 到 1024chain 的跨链 |

#### POST /stake

调用 EVM stake 合约接口，完成从 Arbitrum 到 1024chain 的跨链。

**请求体：**
```json
{
  "amount": "1000000",
  "target_address": "1024chain接收地址"
}
```

**参数说明：**
- `amount`：USDC 金额（字符串格式，最小单位，例如 "1000000" = 1 USDC，假设 6 位小数）
- `target_address`：1024chain 上的接收地址（字符串格式）

**响应示例：**
```json
{
  "success": true,
  "message": "Stake successful",
  "tx_hash": "0x..."
}
```

**错误响应：**
```json
{
  "success": false,
  "message": "错误信息",
  "tx_hash": null
}
```

**功能流程：**
1. 检查中转钱包的 USDC 余额
2. 如果余额不足，返回错误
3. 检查 USDC allowance
4. 如果 allowance 不足，自动调用 `approve` 函数（使用最大授权金额 10^18）
5. 调用 EVM stake 合约接口
6. 等待交易确认
7. 返回交易哈希

**示例请求：**
```bash
curl -X POST http://localhost:8084/stake \
  -H "Content-Type: application/json" \
  -d '{
    "amount": "1000000",
    "target_address": "1024chain接收地址"
  }'
```

**使用 CLI 工具：**
```bash
# 查看帮助
./gateway-cli.sh help

# 质押 USDC
./gateway-cli.sh stake 1000000 "1024chain_receiver_address"

# 使用自定义服务地址
GATEWAY_URL=http://localhost:8084 ./gateway-cli.sh stake 1000000 "address"
```

### 配置参数

| 配置项 | 环境变量 | 默认值 | 说明 |
|--------|----------|--------|------|
| RPC 地址 | `RPC_URL` | 无 | Arbitrum RPC 地址 |
| 私钥 | `PRIVATE_KEY` | 无 | 中转钱包私钥（hex 格式，带或不带 0x 前缀） |
| Bridge 合约地址 | `BRIDGE_CONTRACT_ADDRESS` | 无 | EVM Bridge 合约地址 |
| USDC 合约地址 | `USDC_CONTRACT_ADDRESS` | 无 | USDC ERC20 合约地址 |
| 链 ID | `CHAIN_ID` | 421614 | Arbitrum Sepolia 链 ID |
| 服务端口 | `PORT` | 8084 | HTTP 服务监听端口 |

### 架构说明

**与 Relayer 的区别：**
- **Relayer**：监听链上事件、签名验证、多签提交（双向跨链）
- **Gateway**：接收外部 HTTP 请求，使用中转钱包调用 EVM stake 接口（单向：Arbitrum → 1024chain）

**工作流程：**
1. 用户使用成熟的跨链桥（如 LiFi）将资产从任意链跨链到 Arbitrum
2. USDC 转入中转钱包地址
3. Gateway 服务接收 HTTP 请求
4. 服务使用中转钱包调用 EVM stake 合约接口
5. 完成从 Arbitrum 到 1024chain 的第二步跨链

---

### Withdraw Gateway Service API

Withdraw Gateway Service 提供 HTTP API，用于完成从 Arbitrum 到任意链的跨链转账（Withdraw 方向的第二步）。

#### API 总览表

| 端点 | 方法 | 功能 | 请求参数 | 响应 | 说明 |
|------|------|------|----------|------|------|
| `/withdraw` | POST | 使用 LiFi SDK 执行跨链交易 | `target_chain`, `target_asset`, `usdc_amount`, `recipient_address` | `success`, `message`, `route_id`, `tx_hash` | 完成从 Arbitrum 到任意链的跨链 |

#### POST /withdraw

接收提现请求，使用 LiFi SDK 发起跨链交易，将 Arbitrum 上的 USDC 跨链到目标链的目标资产。

**请求体：**
```json
{
  "target_chain": 1,
  "target_asset": "0x6B175474E89094C44Da98b954EedeAC495271d0F",
  "usdc_amount": "1000000",
  "recipient_address": "0xRecipientAddress"
}
```

**参数说明：**
- `target_chain`：目标链的链 ID（整数，例如：1 = Ethereum Mainnet, 137 = Polygon, 56 = BSC）
- `target_asset`：目标链上资产的合约地址（字符串，hex 格式，必须以 0x 开头）
- `usdc_amount`：要提现的 USDC 数量（字符串格式，最小单位，例如 "1000000" = 1 USDC，假设 6 位小数）
- `recipient_address`：接收资产的目标地址（字符串，hex 格式，必须以 0x 开头）

**响应示例：**
```json
{
  "success": true,
  "message": "Withdrawal initiated",
  "route_id": "route_id_from_lifi",
  "tx_hash": "0x..."
}
```

**错误响应：**
```json
{
  "success": false,
  "message": "错误信息",
  "route_id": null,
  "tx_hash": null
}
```

**功能流程：**
1. 验证请求参数（类型、格式、有效性）
2. 检查速率限制（使用 rate-limiter-flexible）
3. 加入请求队列（使用 p-queue 控制并发）
4. 使用 LiFi SDK 获取跨链报价（`getQuote`）
5. 使用 LiFi SDK 执行跨链路由（`executeRoute`）
6. 跟踪路由执行状态（通过 `updateCallback`）
7. 返回路由 ID 和交易哈希

**支持的链和代币：**
- 支持所有 LiFi SDK 支持的链（Ethereum、Polygon、BSC、Avalanche、Base、Optimism、Arbitrum 等）
- 支持目标链上的任意代币（通过 LiFi SDK 自动路由和兑换）

**示例请求：**
```bash
curl -X POST http://localhost:8085/withdraw \
  -H "Content-Type: application/json" \
  -d '{
    "target_chain": 1,
    "target_asset": "0x6B175474E89094C44Da98b954EedeAC495271d0F",
    "usdc_amount": "1000000",
    "recipient_address": "0xRecipientAddress"
  }'
```

**配置参数：**

| 配置项 | 环境变量 | 默认值 | 说明 |
|--------|----------|--------|------|
| LiFi API 密钥 | `LIFI_API_KEY` | 无 | LiFi API 密钥（可选，用于提高速率限制） |
| Arbitrum RPC | `ARBITRUM_RPC_URL` | 无 | Arbitrum RPC 地址 |
| Arbitrum Chain ID | `ARBITRUM_CHAIN_ID` | 42161 | Arbitrum 链 ID（421614 = Sepolia, 42161 = Mainnet） |
| Arbitrum USDC 地址 | `ARBITRUM_USDC_ADDRESS` | 无 | Arbitrum 上的 USDC 合约地址 |
| 中转钱包私钥 | `TRANSIT_WALLET_PRIVATE_KEY` | 无 | 中转钱包私钥（hex 格式，带或不带 0x 前缀） |
| 默认滑点 | `DEFAULT_SLIPPAGE` | 0.03 | 默认滑点（0.03 = 3%） |
| 服务端口 | `PORT` | 8085 | HTTP 服务监听端口 |
| 环境 | `NODE_ENV` | development | 环境（development 或 production） |
| CORS 允许的源 | `CORS_ALLOWED_ORIGINS` | 无 | 允许的 CORS 源（逗号分隔，可选） |

**速率限制：**
- 未使用 API 密钥：每两小时最多 200 个请求
- 使用 API 密钥：每分钟最多 200 个请求
- 服务内部实现速率限制和并发控制

**架构说明：**

**与 EVM Gateway Service 的区别：**
- **EVM Gateway Service**：处理 Deposit 方向的第二步（Arbitrum → 1024chain），调用 EVM stake 合约
- **Withdraw Gateway Service**：处理 Withdraw 方向的第二步（Arbitrum → 任意链），使用 LiFi SDK 执行跨链

**工作流程：**
1. 用户在 1024chain 调用 SVM stake 合约，将 USDC 发送到 Broker 的 Arbitrum 地址
2. USDC 从 1024chain 跨链到 Arbitrum，转入 Broker 的中转钱包
3. Withdraw Gateway Service 接收 HTTP 请求
4. 服务使用 LiFi SDK 获取跨链报价并执行跨链交易
5. 完成从 Arbitrum 到目标链的跨链

---
4. 服务使用中转钱包调用 EVM stake 合约接口
5. 完成从 Arbitrum 到 1024chain 的第二步跨链

---

## Relayer 服务 API

### 监听功能

Relayer 需要监听两条链的质押事件：

**监听 SVM 事件**：
```typescript
// 连接到 SVM RPC
const connection = new Connection(SVM_RPC_URL);

// 监听 Anchor 事件
program.addEventListener("StakeEvent", (event, slot) => {
  processSvmEvent(event);
});
```

**监听 EVM 事件**：
```typescript
// 连接到 EVM RPC
const provider = new ethers.providers.JsonRpcProvider(EVM_RPC_URL);

// 监听合约事件
bridgeContract.on("StakeEvent", (sourceContract, targetContract, chainId, 
                                  blockHeight, amount, receiverAddress, nonce) => {
  processEvmEvent(...);
});
```

### 签名转发功能（双算法支持）

#### 处理 SVM 事件 → 提交到 EVM

```typescript
async function processSvmEvent(eventData: StakeEventData) {
  // 1. 验证事件
  if (!verifyEventSource(eventData)) return;
  
  // 2. 转换为 EVM 格式（JSON 序列化）
  const jsonData = {
    sourceContract: eventData.sourceContract.toBase58(),
    targetContract: eventData.targetContract.toBase58(),
    chainId: eventData.sourceChainId.toString(),
    blockHeight: eventData.blockHeight.toString(),
    amount: eventData.amount.toString(),
    receiverAddress: eventData.receiverAddress,
    nonce: eventData.nonce.toString()
  };
  const jsonString = JSON.stringify(jsonData);
  
  // 3. 计算哈希（SHA-256 + EIP-191）
  const sha256Hash = crypto.createHash('sha256').update(jsonString).digest();
  const ethSignedHash = ethers.utils.keccak256(
    ethers.utils.concat([
      ethers.utils.toUtf8Bytes('\x19Ethereum Signed Message:\n32'),
      sha256Hash
    ])
  );
  
  // 4. ECDSA 签名
  const signature = await ecdsaWallet.signMessage(ethers.utils.arrayify(ethSignedHash));
  
  // 5. 提交到 EVM 接收端合约
  await evmBridgeContract.submitSignature(eventData, signature);
}
```

#### 处理 EVM 事件 → 提交到 SVM

```typescript
async function processEvmEvent(eventData: EvmStakeEventData) {
  // 1. 验证事件
  if (!verifyEventSource(eventData)) return;
  
  // 2. 转换为 SVM 格式（Borsh 序列化）
  const svmEventData: StakeEventData = {
    sourceContract: new PublicKey(eventData.sourceContract),
    targetContract: new PublicKey(eventData.targetContract),
    sourceChainId: new BN(eventData.chainId),
    targetChainId: new BN(eventData.targetChainId),
    blockHeight: new BN(eventData.blockHeight),
    amount: new BN(eventData.amount),
    receiverAddress: eventData.receiverAddress,
    nonce: new BN(eventData.nonce)
  };
  
  // 3. Borsh 序列化
  const message = program.coder.types.encode("StakeEventData", svmEventData);
  
  // 4. Ed25519 签名
  const signature = await ed25519.sign(message, keypair.secretKey.slice(0, 32));
  
  // 5. 创建 Ed25519Program 验证指令
  const ed25519Ix = Ed25519Program.createInstructionWithPublicKey({
    publicKey: keypair.publicKey.toBytes(),
    message: message,
    signature: signature
  });
  
  // 6. 提交到 SVM 接收端合约（包含 Ed25519 验证指令）
  const tx = await program.methods
    .submitSignature(svmEventData.nonce, svmEventData, Array.from(signature))
    .preInstructions([ed25519Ix]) // 先验证签名
    .rpc();
}
```

### 验证规则

通用验证（两条链相同）：
- `sourceContract`：必须匹配配置中的发送端合约地址
- `chainId`：必须匹配配置中的发送端链 ID
- `nonce`：检查本地缓存，确保 nonce 递增（可选，接收端合约也会验证）

---

## 模块间调用规约

### 用户 → 发送端合约

```
用户调用: stake(amount, receiverAddress)
触发事件: StakeEvent
```

### 发送端合约 → Relayer（事件监听）

```
发送端合约触发: StakeEvent
Relayer 监听: 获取事件数据
```

### Relayer → 接收端合约

```
Relayer 调用: submitSignature(eventData, signature)
接收端验证: 签名合法性、nonce 有效性
接收端执行: 达到阈值后解锁代币
```

### 管理员 → 接收端合约

```
管理员（多签钱包）调用: addRelayer(relayerAddress)
管理员（多签钱包）调用: removeRelayer(relayerAddress)
管理员（多签钱包）调用: addLiquidity(amount)
管理员（多签钱包）调用: withdrawLiquidity(amount)
管理员查询: isRelayer(relayerAddress)
管理员查询: getRelayerCount()
```

**注意：** 所有管理接口使用多签钱包（Squad）调用，合约层面只验证签名，不关心多签逻辑。多签投票在外部处理。

---

## 数据结构

### Solana 账户设计

#### ReceiverState 主账户

存储固定大小的配置数据，支持最多 18 个 relayer：

```rust
pub struct ReceiverState {
    pub vault: Pubkey,              // 32 bytes
    pub admin: Pubkey,              // 32 bytes
    pub relayer_count: u64,         // 8 bytes
    pub source_contract: Pubkey,    // 32 bytes
    pub source_chain_id: u64,       // 8 bytes
    pub target_chain_id: u64,       // 8 bytes
    pub relayers: Vec<Pubkey>,      // 4 + 32 * 18 = 580 bytes
    pub last_nonce: u64,            // 8 bytes (用于nonce递增判断)
}
```

**账户大小：** ~708 bytes（在 10KB 限制内）

**设计说明：**
- `relayers`: 最多支持 18 个 relayer
- `last_nonce`: 记录最后一个已使用的 nonce，用于判断新 nonce 是否递增
- Nonce 使用 64 位无符号整数（u64），通过递增判断来防止重放攻击

#### CrossChainRequest PDA 账户

为每个跨链请求（nonce）创建独立的 PDA 账户来存储 relayer 签名缓存：

```rust
pub struct CrossChainRequest {
    pub nonce: u64,                    // 8 bytes
    pub signed_relayers: Vec<Pubkey>,   // 4 + 32 * 18 = 580 bytes
    pub signature_count: u8,            // 1 byte
    pub is_unlocked: bool,              // 1 byte
    pub event_data: StakeEventData,     // 事件数据（用于验证和转账）
}
```

**PDA 种子：** `[b"cross_chain_request", nonce.to_le_bytes()]`  
**账户大小：** ~600+ bytes（固定大小）

**设计优势：**
- **支持至少 100 个未完成的请求**：每个请求独立账户，可同时存在 100+ 个未完成的请求
- **支持 1200 个签名缓存**：100 个请求 × 18 个 relayer = 1800 个签名（超过要求的 1200 个）
- 支持理论上无限次请求（每个 nonce 独立账户）
- 支持最多 18 个 relayer
- 固定大小，易于管理
- 解锁后可以关闭账户回收租金

### 密码学算法说明

系统采用**各自原生密码学算法**的设计原则，最大化安全性和性能：

#### SVM 端（Solana/1024chain）

**签名算法**：Ed25519（Solana 原生）

```typescript
// 1. Borsh 序列化事件数据
const message = program.coder.types.encode("StakeEventData", eventData);

// 2. Ed25519 签名
const signature = await ed25519.sign(message, keypair.secretKey.slice(0, 32));

// 3. 验证（通过 Ed25519Program）
const ed25519Ix = Ed25519Program.createInstructionWithPublicKey({
  publicKey: keypair.publicKey.toBytes(),
  message: message,
  signature: signature
});
```

**特点**：
- 签名长度：64 字节
- 公钥长度：32 字节
- 无需额外哈希（Ed25519 内置）
- 使用 Solana Ed25519Program 预编译合约验证

#### EVM 端（Ethereum/Arbitrum）

**签名算法**：ECDSA (secp256k1) + EIP-191

```solidity
// 1. JSON 序列化
string memory json = serializeToJSON(eventData);

// 2. SHA-256 哈希
bytes32 sha256Hash = sha256(bytes(json));

// 3. EIP-191 格式
bytes32 ethSignedHash = keccak256(
    abi.encodePacked("\x19Ethereum Signed Message:\n32", sha256Hash)
);

// 4. ECDSA 验证
address recovered = ecrecover(ethSignedHash, v, r, s);
require(recovered == expectedSigner, "Invalid signature");
```

**特点**：
- 签名长度：65 字节 (r: 32, s: 32, v: 1)
- 地址长度：20 字节
- 两层哈希：SHA-256 + Keccak256 (EIP-191)
- 使用 ecrecover 预编译合约验证

---

## 配置参数

### 网络配置

- **Arbitrum Sepolia RPC**: `https://sepolia-rollup.arbitrum.io/rpc`
- **1024chain Testnet RPC**: https://rpc-testnet.1024chain.com/rpc/

- **Arbitrum Mainnet RPC**: `https://arb1.arbitrum.io/rpc`
- **1024chain Mainnet RPC**: （待配置）

### Chain ID

- **Arbitrum Sepolia**: 421614
- **Arbitrum Mainnet**: 42161
- **1024chain Testnet**: （待确认）
- **1024chain Mainnet**: （待确认）

### 合约地址

根据部署网络进行配置：
- 发送端合约地址
- 接收端合约地址
- 质押金库地址（EVM）
- 质押金库地址（SVM）
- 管理员地址（EVM）
- 管理员地址（SVM）

### USDC代币地址配置

**SVM端（1024chain）：**
- USDC mint account地址：需要在部署时配置，通过 `configure_usdc` 函数设置
- 测试网USDC mint地址：待确认
- 主网USDC mint地址：待确认

**EVM端（Arbitrum）：**
- USDC ERC20合约地址：需要在部署时配置，通过 `configure_usdc` 函数设置
- Arbitrum Sepolia USDC地址：待确认
- Arbitrum Mainnet USDC地址：`0xaf88d065e77c8cC2239327C5EDb3A432268e5831`（参考，需确认）

### Relayer 配置

- Relayer 数量：≥ 3，最多 18 个
- 签名阈值：`Math.ceil(relayer_count * 2 / 3)` （当签名数 >= 阈值时解锁）
- Relayer 私钥列表：每个 relayer 独立保管

**阈值示例：**
- 3 个 Relayer → 阈值 2
- 4 个 Relayer → 阈值 3
- 5 个 Relayer → 阈值 4
- 18 个 Relayer → 阈值 12

### Nonce 处理

- **Nonce 类型**：64 位无符号整数（u64）
- **Nonce 递增判断**：新 nonce 必须大于 `last_nonce`，否则视为重放攻击
- **Nonce 溢出处理**：当 nonce 达到 `u64::MAX` (18,446,744,073,709,551,615) 时，自动重置为 0
- **未完成请求支持**：至少支持 100 个未完成的跨链请求同时存在
- **签名缓存容量**：至少支持 1200 个签名缓存（100 个请求 × 18 个 relayer = 1800 个签名）
