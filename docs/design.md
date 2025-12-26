# 系统设计文档

## 目录

- [系统架构](#系统架构)
- [密码学算法设计](#密码学算法设计)
- [数据存储设计](#数据存储设计)
- [SVM 合约设计](#svm-合约设计)
- [EVM 合约设计](#evm-合约设计)
- [Gateway 服务设计](#gateway-服务设计)
- [Relayer 服务设计](#relayer-服务设计)
- [安全考虑](#安全考虑)

---

## 系统架构

### 整体架构

系统由三个主要组件组成：

1. **SVM 智能合约**（1024chain）：发送端和接收端合约
2. **EVM 智能合约**（Arbitrum Sepolia）：发送端和接收端合约
3. **Relayer 中继服务**：监听事件、签名验证、交易提交

### 跨链转账流程

```
用户 → 发送端合约（质押） → 触发事件 → Relayer监听 → 签名验证 → 接收端合约（解锁）
```

### 多签钱包与 PDA 金库架构

**设计原则：**
- **管理操作**：使用 Squad 多签钱包，提高安全性
- **业务操作**：使用 PDA 金库，提高效率
- **职责分离**：管理操作由多签控制（外部处理），业务操作由 PDA 自动执行（合约控制）

**架构优势：**
- **性能优化**：解锁操作使用 PDA，无需多签投票，快速执行
- **安全性**：管理接口多签保护，防止单点故障；金库资金 PDA 控制，只能通过合约逻辑操作
- **灵活性**：可以随时增加/减少金库流动性；多签成员可以变更（外部处理）
- **简洁性**：合约逻辑保持简洁，不处理多签提案

**Squad 多签程序：**
- 程序地址：`SMPLecH534NA9acB4bMolv7X6RBpK4rjn3LkN1gZXYjy`
- 用于管理操作的多签钱包创建和管理
- 多签投票在外部处理，合约不关心多签逻辑

---

## 密码学算法设计

### 设计原则

**关键设计决策**：SVM 和 EVM 各自使用其原生的密码学算法，以最大化安全性和性能。

### SVM 端（Solana/1024chain）

**签名算法**：Ed25519（Solana 原生）
- 使用 Solana 的 Ed25519Program 预编译合约进行签名验证
- 程序 ID：`Ed25519SigVerify111111111111111111111111111`
- 签名长度：64 字节
- 公钥长度：32 字节

**数据序列化**：Borsh（Solana 原生）
- 使用 Anchor 框架的 Borsh 编码
- 直接序列化 `StakeEventData` 结构体
- 确保字节级别的一致性

**哈希算法**：无需额外哈希
- Ed25519 签名直接对序列化后的数据进行
- 签名本身已包含消息完整性验证

**验证流程**：
1. 客户端使用 `Ed25519Program.createInstructionWithPublicKey()` 创建签名验证指令
2. Solana 运行时在交易执行前验证签名
3. 合约通过 Instructions Sysvar 检查 Ed25519Program 指令的存在和正确性
4. 验证参数匹配：签名、公钥、消息内容

### EVM 端（Ethereum/Arbitrum）

**签名算法**：ECDSA (secp256k1)（Ethereum 原生）
- 使用 `ecrecover` 预编译合约进行签名验证
- 签名长度：65 字节 (r: 32, s: 32, v: 1)
- 曲线：secp256k1（与比特币、以太坊相同）

**数据序列化**：JSON 字符串格式
- 将事件数据序列化为 JSON 字符串
- 格式：`{"sourceContract":"...","targetContract":"...","chainId":"...","blockHeight":"...","amount":"...","sender":"...","receiverAddress":"...","nonce":"..."}`
- 确保跨语言的可读性和一致性

**哈希算法**：两层哈希
1. **第一层 - SHA-256**：对 JSON 序列化的事件数据进行哈希
   ```solidity
   bytes32 dataHash = sha256(jsonString);
   ```
2. **第二层 - Keccak256 (EIP-191)**：应用 EIP-191 签名消息格式
   ```solidity
   bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", dataHash));
   ```

**验证流程**：
1. 对事件数据进行 JSON 序列化
2. 计算 SHA-256 哈希
3. 应用 EIP-191 前缀并计算 Keccak256 哈希
4. 使用 `ecrecover` 从签名中恢复签名者地址
5. 验证恢复的地址与预期的 Relayer 地址匹配

### Relayer 签名职责

由于两条链使用不同的密码学算法，Relayer 需要根据目标链选择相应的签名方式：

#### 监听 SVM 事件 → 提交到 EVM

1. 监听 SVM 链的 `StakeEvent` 事件
2. 获取事件数据（Borsh 格式）
3. **转换为 EVM 格式**：
   - 将事件数据序列化为 JSON 字符串
   - 计算 SHA-256 哈希
   - 应用 EIP-191 格式
4. **使用 ECDSA (secp256k1) 私钥签名**
5. 提交签名到 EVM 接收端合约

#### 监听 EVM 事件 → 提交到 SVM

1. 监听 EVM 链的 `StakeEvent` 事件
2. 获取事件数据（EVM event logs）
3. **转换为 SVM 格式**：
   - 构造 `StakeEventData` 结构体
   - 使用 Borsh 序列化
4. **使用 Ed25519 私钥签名**
5. 创建 `Ed25519Program` 验证指令
6. 提交签名到 SVM 接收端合约

### 密码学算法对比

| 特性 | SVM (Solana) | EVM (Ethereum) |
|------|--------------|----------------|
| **签名算法** | Ed25519 | ECDSA (secp256k1) |
| **签名长度** | 64 字节 | 65 字节 |
| **公钥长度** | 32 字节 | 20 字节（地址） |
| **数据序列化** | Borsh（二进制） | JSON（字符串） |
| **哈希算法** | 无需额外哈希 | SHA-256 + Keccak256 |
| **签名格式** | 标准 Ed25519 | EIP-191 |
| **验证方式** | Ed25519Program | ecrecover |
| **性能** | 极快（原生支持） | 快（预编译） |
| **安全级别** | 128 位（Ed25519） | 128 位（secp256k1） |

### 为什么使用各自原生算法？

1. **性能最优**：使用链的原生算法可以充分利用预编译合约，获得最佳性能
2. **安全性最高**：经过充分测试和审计的原生实现，安全性最可靠
3. **Gas 成本最低**：原生算法的 Gas 消耗最少
4. **生态兼容性**：与各链生态的标准工具和钱包完全兼容
5. **简化实现**：无需在合约中实现复杂的密码学库

### 跨链兼容性保证

虽然两条链使用不同的签名算法，但通过以下机制保证跨链兼容性：

1. **事件数据结构统一**：两条链使用相同的 `StakeEventData` 结构
2. **Relayer 转换层**：Relayer 负责在两种格式之间转换
3. **独立验证**：每条链独立验证其原生格式的签名
4. **Nonce 机制统一**：两条链使用相同的 nonce 递增判断机制防重放
5. **阈值计算统一**：两条链使用相同的 2/3 阈值计算公式

---

## 数据存储设计

### Solana (SVM) 账户设计

由于 Solana 账户大小限制（最大 10MB，但实际使用中建议不超过 10KB），需要采用 PDA（Program Derived Address）来支持无限请求和最多 18 个 relayer。

#### ReceiverState 主账户

存储固定大小的配置数据：

```rust
pub struct ReceiverState {
    pub vault: Pubkey,              // 32 bytes (PDA金库地址)
    pub admin: Pubkey,              // 32 bytes (多签钱包地址)
    pub relayer_count: u64,         // 8 bytes
    pub source_contract: Pubkey,     // 32 bytes
    pub source_chain_id: u64,       // 8 bytes
    pub target_chain_id: u64,       // 8 bytes
    pub relayers: Vec<Pubkey>,      // 4 + 32 * 18 = 580 bytes (最多18个relayer)
    pub last_nonce: u64,            // 8 bytes (用于nonce递增判断)
}
```

**设计说明：**
- `vault`: PDA金库地址，由程序控制，支持自动转账
- `admin`: 多签钱包地址，用于管理操作（合约层面只验证签名，不关心多签逻辑）

**账户大小计算：**
- Base: 120 bytes
- Relayers (18个): 580 bytes
- Last nonce: 8 bytes
- **总计: ~708 bytes** (在 10KB 限制内)

**设计说明：**
- `relayers`: 最多支持 18 个 relayer
- `last_nonce`: 记录最后一个已使用的 nonce，用于判断新 nonce 是否递增
- Nonce 使用 64 位无符号整数（u64），通过递增判断来防止重放攻击
- 当 nonce 溢出时（达到 u64::MAX），重置为 0

#### CrossChainRequest PDA 账户

为每个跨链请求（nonce）创建独立的 PDA 账户来存储 relayer 签名缓存：

```rust
pub struct CrossChainRequest {
    pub nonce: u64,                    // 8 bytes
    pub signed_relayers: Vec<Pubkey>,   // 4 + 32 * 18 = 580 bytes (最多18个relayer)
    pub signature_count: u8,            // 1 byte
    pub is_unlocked: bool,              // 1 byte
    pub event_data: StakeEventData,     // 事件数据（用于验证和转账）
}
```

**PDA 种子：** `[b"cross_chain_request", nonce.to_le_bytes()]`

**账户大小：** ~600+ bytes（固定大小，支持最多 18 个 relayer）

**设计优势：**
- **支持至少 100 个未完成的请求**：每个请求独立账户，可同时存在 100+ 个未完成的请求
- **支持 1200 个签名缓存**：100 个请求 × 18 个 relayer = 1800 个签名（超过要求的 1200 个）
- 每个请求独立账户，支持无限 nonce
- 固定大小，易于管理
- 解锁后可以关闭账户回收租金

**Nonce 递增判断逻辑：**
1. 新 nonce 必须大于 `last_nonce`（递增）
2. 如果新 nonce <= `last_nonce`，则视为重放攻击，拒绝处理
3. 当 nonce 达到 `u64::MAX` 时，重置为 0（溢出处理）
4. 解锁成功后，更新 `last_nonce = nonce`

#### 设计权衡

| 方案 | 优点 | 缺点 |
|------|------|------|
| **当前方案（PDA）** | 支持无限 nonce，固定大小 | 需要为每个 nonce 创建账户 |
| Vec 存储所有记录 | 简单直接 | 账户大小会无限增长 |
| 位图跟踪 | 空间效率高 | 实现复杂，需要压缩算法 |

**选择 PDA 方案的原因：**
1. Solana 账户大小限制严格（10KB）
2. 需要支持理论上无限次请求
3. 每个 nonce 的签名记录是独立的，适合分离存储
4. 解锁后可以关闭账户，回收租金

### EVM 合约存储设计

EVM 合约使用映射（mapping）存储，无大小限制：

```solidity
struct NonceSignature {
    mapping(address => bool) signedRelayers;
    uint8 signatureCount;
    bool isUnlocked;
}

mapping(uint256 => NonceSignature) public nonceSignatures;
mapping(uint256 => bool) public usedNonces;
```

---

## SVM 合约设计

### 统一初始化设计

**重要变更：** 一个平台的接收端和发送端初始化函数合并为一个 `initialize` 指令。

在初始化时，同时创建 `SenderState` 和 `ReceiverState` 账户，共享相同的配置（vault、admin、chain_id 等）。

#### 初始化账户结构

```rust
// 发送端状态
pub struct SenderState {
    pub vault: Pubkey,
    pub admin: Pubkey,
    pub usdc_mint: Pubkey,              // USDC mint account地址（SVM）或合约地址（EVM）
    pub nonce: u64,                     // 64位无符号整数，初始为0
    pub target_contract: Pubkey,
    pub source_chain_id: u64,
    pub target_chain_id: u64,
}

// 接收端状态
pub struct ReceiverState {
    pub vault: Pubkey,                  // 与发送端共享
    pub admin: Pubkey,                  // 与发送端共享
    pub usdc_mint: Pubkey,              // USDC mint account地址（SVM）或合约地址（EVM），与发送端共享
    pub relayer_count: u64,
    pub source_contract: Pubkey,
    pub source_chain_id: u64,
    pub target_chain_id: u64,
    pub relayers: Vec<Pubkey>,          // 最多18个
    pub last_nonce: u64,                // 用于nonce递增判断
}
```

#### 金库设计（PDA）

**重要设计决策：** 金库使用 PDA（Program Derived Address）而不是普通钱包或多签钱包。

**PDA 种子：** `[b"vault"]`

**设计优势：**
- **自动转账**：合约可以直接控制 PDA，无需外部签名即可执行转账
- **安全性**：PDA 由程序控制，无法被外部直接操作
- **解锁效率**：达到阈值后立即执行解锁，无需等待多签投票
- **简化实现**：无需处理多签提案机制，保持合约逻辑简洁

**Token Account PDA：**
- 种子：`[b"vault_token", usdc_mint]`
- 所有者：vault PDA
- 用途：存储 USDC 代币

#### 管理钱包设计（多签）

**重要设计决策：** 所有管理接口使用多签钱包调用，但合约层面不关心多签逻辑。

**设计说明：**
- `admin` 字段存储多签钱包地址
- 合约只验证 `admin` 签名，不关心是否是多签
- 多签逻辑在外部（Squad 程序）处理
- 管理接口包括：`initialize`, `configure_usdc`, `configure_peer`, `add_relayer`, `remove_relayer`, `add_liquidity`, `withdraw_liquidity`

**优势：**
- 合约保持简洁，无需处理多签提案
- 管理操作需要多签保护，提高安全性
- 向后兼容，现有代码基本不需要修改

#### 主要功能

1. **initialize**: 统一初始化发送端和接收端合约
   - 创建 `SenderState` 账户
   - 创建 `ReceiverState` 账户
   - 创建 PDA 金库（vault）和对应的 token account
   - 设置 admin 为多签钱包地址
   - 初始化 nonce 为 0
   - 初始化 last_nonce 为 0

2. **configure_usdc**: 配置USDC代币地址
   - 同时配置发送端和接收端的 `usdc_mint` 字段
   - **SVM端**：传入USDC的SPL Token mint account地址
   - **EVM端**：传入USDC的ERC20合约地址
   - 因为两端使用同一个USDC代币，所以共享相同的地址

3. **configure_peer**: 统一配置对端合约和链ID
   - 同时配置发送端的 `target_contract`、`source_chain_id`、`target_chain_id`
   - 同时配置接收端的 `source_contract`、`source_chain_id`、`target_chain_id`
   - 因为对端是同一个，所以两个配置共享相同的参数

4. **stake** (发送端): 质押代币并触发事件
   - **验证USDC地址已配置**：如果 `sender_state.usdc_mint` 为无效地址（如 `Pubkey::default()`），返回错误
   - Nonce 递增逻辑：
     - 当前 nonce = `sender_state.nonce`
     - 新 nonce = `current_nonce + 1`
     - 如果 `new_nonce == 0`（溢出），重置为 0
     - 更新 `sender_state.nonce = new_nonce`

5. **add_relayer** / **remove_relayer** (接收端): Relayer 白名单管理
   - **重要变更**: 移除了`ecdsa_pubkey`参数，直接使用relayer的Solana公钥（Ed25519）
   - `add_relayer(relayer: Pubkey)` - 只需要relayer地址
   - Relayer的Solana密钥就是Ed25519密钥，无需额外存储

6. **submit_signature** (接收端): 提交签名并检查阈值
   - **验证USDC地址已配置**：如果 `receiver_state.usdc_mint` 为无效地址（如 `Pubkey::default()`），返回错误
   - **Ed25519签名验证**：使用Solana的Ed25519Program进行真实的密码学验证
   - 客户端必须在交易中包含`Ed25519Program.createInstructionWithPublicKey()`指令
   - 合约检查Instructions Sysvar，确认Ed25519Program验证通过
   - 解锁操作使用 PDA 金库作为 authority，自动执行转账

7. **add_liquidity** (接收端): 增加流动性
   - 从多签钱包（admin）转账到 PDA 金库
   - 需要 admin 签名（多签钱包）
   - 用于向金库注入资金

8. **withdraw_liquidity** (接收端): 提取流动性
   - 从 PDA 金库转账到多签钱包（admin）
   - 需要 admin 签名（多签钱包）
   - 使用 PDA 作为 authority 执行转账
   - 用于从金库提取资金

#### submit_signature 流程

```
1. 验证 relayer 在白名单中
2. 验证USDC地址已配置：如果 usdc_mint 为无效地址，返回错误 "USDC address not configured"
3. 验证源链合约地址和 chain ID
4. 检查 nonce 是否递增：
   - 如果 nonce <= last_nonce，拒绝（重放攻击）
   - 如果 nonce > last_nonce，继续处理
5. 获取或创建 CrossChainRequest PDA 账户
6. **初始化或验证 event_data 一致性**：
   - 如果这是第一个签名（signatureCount == 0）：
     * 将传入的 event_data 存储为"标准答案"
     * 这是后续所有 relayer 必须遵循的 event_data
   - 如果不是第一个签名（signatureCount > 0）：
     * **关键安全机制**：验证传入的 event_data 是否与已存储的 event_data 完全一致
     * 检查所有字段：sourceContract, targetContract, sourceChainId, targetChainId, 
                     blockHeight, amount, sender, receiverAddress, nonce
     * 如果任何字段不匹配，拒绝并返回错误 "Invalid event data"
     * 这防止恶意 relayer 提交不同的 event_data 导致数据不一致
7. 检查该 relayer 是否已为此 nonce 签名
8. **验证Ed25519签名**（真实密码学验证）：
   - 从Instructions Sysvar加载当前指令之前的所有指令
   - 查找Ed25519Program指令（程序ID: Ed25519SigVerify111111111111111111111111111）
   - 验证Ed25519Program指令中的签名、公钥、消息与我们的参数匹配
   - 验证签名是否匹配传入的 event_data
   - 如果找到匹配的Ed25519Program指令，说明签名已被密码学验证
   - 如果没有找到或不匹配，拒绝签名
9. 记录签名到 CrossChainRequest.signed_relayers
10. 计算签名数量，检查是否达到阈值（> 2/3 relayer_count）
11. 如果达到阈值：
   - 从 PDA 金库转账到接收地址（使用配置的 usdc_mint）
   - **重要**：使用存储的 event_data（第一个 relayer 提交的）而不是函数参数
   - 使用 PDA 作为 authority，无需外部签名
   - 更新 last_nonce = 存储的 event_data.nonce（标记为已使用）
   - 标记 CrossChainRequest.is_unlocked = true
   - 可选：关闭 CrossChainRequest 账户回收租金
```

#### Event Data 一致性验证机制

**设计原则：以第一个提交的 event_data 为准**

系统采用"第一个提交者决定"的设计原则：
- 第一个 relayer 提交的 `event_data` 会被存储并成为"标准答案"
- 后续所有 relayer 必须提交完全相同的 `event_data` 才能通过验证
- 解锁时使用存储的 `event_data`，确保一致性

**安全性分析：**

1. **防止恶意 relayer 提交错误数据**：
   - 如果第一个 relayer 提交错误的 `event_data`（如错误的 amount 或 receiver）
   - 正常 relayer 无法提交不同的 `event_data`（会被一致性检查拒绝）
   - 正常 relayer 也无法提交相同的错误 `event_data`（因为他们的签名是对正确数据的签名，签名验证会失败）
   - 结果：无法达到阈值，流程会卡住，不会执行错误的解锁

2. **正常情况下的工作流程**：
   - 所有正常 relayer 监听链上事件，获取相同的正确 `event_data`
   - 第一个 relayer 提交正确的 `event_data` 并存储
   - 后续 relayer 提交相同的正确 `event_data`，通过一致性检查
   - 每个 relayer 的签名都匹配他们提交的 `event_data`，通过签名验证
   - 达到阈值后，使用存储的正确 `event_data` 解锁

3. **安全性保证**：
   - ✅ 需要 >2/3 的 relayer 签名才能解锁
   - ✅ 所有 relayer 必须对相同的 `event_data` 签名
   - ✅ 签名验证确保每个 relayer 的签名匹配其提交的 `event_data`
   - ✅ 一致性检查确保所有 relayer 提交相同的 `event_data`
   - ✅ 即使第一个 relayer 是恶意的，正常 relayer 无法通过签名验证来确认错误的 `event_data`

**潜在攻击场景与防护：**

- **场景1**：第一个 relayer 提交错误的 `event_data`
  - 正常 relayer 无法提交不同的数据（一致性检查拒绝）
  - 正常 relayer 无法提交相同的错误数据（签名验证失败）
  - **结果**：无法达到阈值，系统安全

- **场景2**：多个 relayer（< 阈值）被劫持并提交错误的 `event_data`
  - 如果第一个 relayer 正常，后续恶意 relayer 无法提交不同的数据
  - 如果第一个 relayer 恶意但数量不足，正常 relayer 无法通过签名验证
  - **结果**：无法达到阈值，系统安全

- **场景3**：超过 2/3 的 relayer 被同时劫持
  - 这是系统威胁模型假设的攻击场景
  - 如果超过 2/3 的 relayer 被劫持，系统无法防止恶意行为
  - 这是所有多重签名系统的固有风险

#### Ed25519签名验证详解

**为什么使用Ed25519Program？**
- Solana BPF程序无法直接使用ed25519-dalek等库（需要std或getrandom）
- Ed25519Program是Solana原生预编译合约，提供高效的Ed25519验证
- 程序ID: `Ed25519SigVerify111111111111111111111111111`

**工作原理：**
1. 客户端创建两个指令：
   - `Ed25519Program.createInstructionWithPublicKey()` - 执行密码学验证
   - `submit_signature()` - 执行业务逻辑
2. Ed25519Program在交易执行前先验证签名
3. 我们的合约通过Instructions Sysvar检查Ed25519Program指令
4. 验证指令中的参数（签名、公钥、消息）与我们的匹配
5. 如果交易成功执行到这里，说明签名验证通过

**安全性：**
- ✅ 真实的密码学验证，无法伪造签名
- ✅ 与Solana交易签名相同的安全级别
- ✅ 防止恶意relayer提交虚假签名
- ✅ 结合白名单+2/3阈值提供多层保护

#### Nonce 溢出处理

当 nonce 达到 `u64::MAX` (18,446,744,073,709,551,615) 时：
- 下一次 `stake` 调用时，nonce 会溢出并重置为 0
- 此时需要特殊处理：如果 `last_nonce` 接近 `u64::MAX`，允许 nonce 从 0 开始
- 实现逻辑：
  ```rust
  let new_nonce = sender_state.nonce.wrapping_add(1);
  if new_nonce == 0 && sender_state.nonce != u64::MAX {
      // 异常情况，不应该发生
      return Err(Error::NonceOverflow);
  }
  sender_state.nonce = new_nonce;
  ```

---

## EVM 合约设计

### 金库和管理员地址设计

**重要设计决策：** EVM 合约中的金库地址（vault）和管理员地址（admin）**完全支持多签钱包**（如 Gnosis Safe），合约层面将其视为普通地址。

**设计说明：**
- **多签钱包兼容性**：在 EVM 中，多签钱包（如 Gnosis Safe）实际上就是一个普通的 `address` 类型
- **合约透明性**：从合约的角度来看，多签钱包和普通 EOA（Externally Owned Account）没有区别
- **权限检查**：合约只需要验证 `msg.sender == admin` 或 `msg.sender == vault`，不需要关心它们是否是多签
- **多签逻辑外部化**：多签的投票、阈值检查等逻辑在外部处理（Gnosis Safe 合约），当多签通过后，会以多签钱包地址的身份调用我们的合约

**优势：**
- **向后兼容**：支持普通 EOA 和多签钱包，无需修改合约代码
- **安全性提升**：管理员和金库可以使用多签钱包，提高资金和管理操作的安全性
- **实现简洁**：合约保持简洁，无需处理多签提案机制
- **灵活性**：部署时可以选择使用普通地址或多签地址，根据安全需求灵活配置

**使用示例：**
```solidity
// 初始化时，admin 可以是多签钱包地址，vault 自动设置为合约本身
initialize(
    adminAddress   // 可以是 Gnosis Safe 多签钱包地址
);

// 权限检查时，合约只验证地址，不关心是否是多签
modifier onlyAdmin() {
    require(msg.sender == admin, "Only admin");
    _;
}

// 转账时，从金库（可能是多签钱包）转账
IERC20(usdcContract).transferFrom(vault, receiver, amount);
```

### 发送端合约

与 SVM 发送端合约功能相同，使用 Solidity 实现。

**账户结构：**
```solidity
struct SenderState {
    address vault;              // 金库地址（可以是多签钱包）
    address admin;              // 管理员地址（可以是多签钱包）
    address usdcContract;        // USDC ERC20合约地址（未配置时为address(0)）
    uint64 nonce;
    address targetContract;
    uint64 sourceChainId;
    uint64 targetChainId;
}
```

**stake 函数验证：**
- 检查 `usdcContract != address(0)`，否则返回错误 "USDC address not configured"
- 从用户地址转账到金库地址（`vault`），金库可以是多签钱包

### 接收端合约

使用 mapping 存储签名记录，无大小限制。

**账户结构：**
```solidity
struct ReceiverState {
    address vault;              // 金库地址（可以是多签钱包）
    address admin;              // 管理员地址（可以是多签钱包）
    address usdcContract;        // USDC ERC20合约地址（未配置时为address(0)）
    uint64 relayerCount;
    address sourceContract;
    uint64 sourceChainId;
    uint64 targetChainId;
    address[] relayers;         // 最多18个
    uint64 lastNonce;
}
```

**submit_signature 函数验证：**
- 检查 `usdcContract != address(0)`，否则返回错误 "USDC address not configured"
- 达到阈值后，从金库地址（`vault`）转账到接收地址，金库可以是多签钱包

### 配置接口

1. **initialize**: 初始化合约（设置vault和admin，可以是多签钱包地址）
2. **configure_usdc**: 配置USDC ERC20合约地址（必须在使用前配置）
3. **configure_peer**: 配置对端合约和链ID

---

## 两段式跨链桥设计

### 概述

系统采用**两段式跨链桥**架构，通过成熟的跨链桥（LiFi SDK）和自定义 Broker 服务，实现任意链与 1024chain 之间的跨链转账。该设计充分利用现有跨链基础设施，降低开发复杂度，同时提供灵活的跨链能力。

### 架构设计

#### 整体流程

**Deposit 方向（存入）：任意链 → Arbitrum → 1024chain**

```
用户钱包（源链代币）
  ↓ [第一步：LiFi SDK 跨链]
Broker 中转钱包（Arbitrum USDC）
  ↓ [第二步：Broker EVM Gateway Service]
EVM Stake 合约（Arbitrum）
  ↓ [Relayer 多签验证]
1024chain 用户地址（USDC）
```

**Withdraw 方向（提取）：1024chain → Arbitrum → 任意链**

```
1024chain 用户地址（USDC）
  ↓ [第一步：SVM Stake 合约]
Broker 中转钱包（Arbitrum USDC）
  ↓ [第二步：Broker Withdraw Gateway Service + LiFi SDK]
用户钱包（目标链目标代币）
```

#### 设计优势

1. **利用成熟基础设施**：使用 LiFi SDK 处理复杂的跨链路由和流动性聚合
2. **降低开发成本**：无需自行实现多链跨链桥，专注于核心业务逻辑
3. **灵活扩展**：支持任意链和任意代币的跨链
4. **统一接口**：通过 Broker 服务提供统一的 HTTP API
5. **安全性**：Broker 服务使用中转钱包，私钥安全管理

#### 支持的链和代币

**Deposit 方向支持的源链：**
- Ethereum Mainnet
- Polygon
- BSC (Binance Smart Chain)
- Avalanche
- Base
- Optimism
- Arbitrum

**Withdraw 方向支持的目标链：**
- 所有 LiFi SDK 支持的链（包括上述所有链）

**代币支持：**
- Deposit：源链上的任意代币（通过 LiFi SDK 自动兑换为 Arbitrum USDC）
- Withdraw：目标链上的任意代币（通过 LiFi SDK 从 Arbitrum USDC 兑换）

### Broker 服务架构

Broker 服务由两个独立的网关服务组成：

1. **EVM Gateway Service**：处理 Deposit 方向的第二步（Arbitrum → 1024chain）
2. **Withdraw Gateway Service**：处理 Withdraw 方向的第二步（Arbitrum → 任意链）

两个服务完全独立，可以分别部署和扩展。

---

## Gateway 服务设计

### 概述

Gateway 服务是一个独立的 HTTP 服务，用于完成跨链桥的第二步：从 Arbitrum 到 1024chain。与 Relayer 服务不同，Gateway 服务不监听链上事件，而是接收外部 HTTP 请求。

### 架构设计

**与 Relayer 的区别：**
- **Relayer**：监听链上事件、签名验证、多签提交（双向跨链）
- **Gateway**：接收外部 HTTP 请求，使用中转钱包调用 EVM stake 接口（单向：Arbitrum → 1024chain）

**工作流程：**
1. 用户使用成熟的跨链桥（如 LiFi）将资产从任意链跨链到 Arbitrum
2. USDC 转入中转钱包地址
3. Gateway 服务接收 HTTP 请求（参数：USDC 金额、目标地址）
4. 服务使用中转钱包调用 EVM stake 合约接口
5. 完成从 Arbitrum 到 1024chain 的第二步跨链

### EVM Gateway Service 设计

#### 功能模块

1. **HTTP API 服务器**
   - 使用 Axum 框架（Rust）
   - 接收 POST `/stake` 请求
   - 请求参数：`amount`（USDC 金额，字符串格式）、`target_address`（1024chain 接收地址）
   - 返回：`success`、`message`、`tx_hash`

2. **USDC 余额检查**
   - 自动检查中转钱包的 USDC 余额
   - 如果余额不足，返回错误信息

3. **USDC 授权管理**
   - 自动检查 USDC allowance
   - 如果 allowance 不足，自动调用 `approve` 函数
   - 使用最大授权金额（10^18），避免频繁 approve 操作

4. **交易发送**
   - 使用 Mutex 序列化交易发送，避免 nonce 冲突和余额检查竞态
   - 调用 EVM stake 合约接口
   - 等待交易确认并返回交易哈希

#### 技术栈

- **异步运行时**：Tokio 1.35
- **HTTP 框架**：Axum 0.7
- **EVM 交互**：Ethers-rs 2.0
- **日志系统**：tracing + tracing-subscriber
- **配置管理**：dotenvy（环境变量）

#### 配置参数

- `RPC_URL`：Arbitrum RPC 地址
- `PRIVATE_KEY`：中转钱包私钥（hex 格式，带或不带 0x 前缀）
- `BRIDGE_CONTRACT_ADDRESS`：Bridge 合约地址
- `USDC_CONTRACT_ADDRESS`：USDC ERC20 合约地址
- `CHAIN_ID`：链 ID（默认 421614，Arbitrum Sepolia）
- `PORT`：HTTP 服务端口（默认 8084）

#### 安全考虑

1. **私钥管理**：私钥通过环境变量配置，不应硬编码
2. **并发控制**：使用 Mutex 序列化关键操作，避免 nonce 冲突
3. **余额检查**：每次请求前检查余额，防止余额不足的交易
4. **授权管理**：使用最大授权金额，减少 approve 操作频率

#### Withdraw Gateway Service 设计

**功能模块：**

1. **HTTP API 服务器**
   - 使用 Express 框架（TypeScript/Node.js）
   - 接收 POST `/withdraw` 请求
   - 请求参数：`target_chain`（目标链 ID）、`target_asset`（目标代币地址）、`usdc_amount`（USDC 金额）、`recipient_address`（接收地址）
   - 返回：`success`、`message`、`route_id`、`tx_hash`

2. **LiFi SDK 集成**
   - 使用 `@lifi/sdk` 获取跨链报价
   - 使用 `executeRoute` 执行跨链交易
   - 支持从 Arbitrum USDC 到任意链任意代币的跨链

3. **速率限制和并发控制**
   - 使用 `rate-limiter-flexible` 实现速率限制
   - 使用 `p-queue` 控制并发请求数量
   - 支持 LiFi API 密钥以提高速率限制

4. **CORS 配置**
   - 开发环境允许所有源
   - 生产环境支持配置允许的源列表
   - 支持通过环境变量配置

**技术栈：**
- **运行时**：Node.js 18+
- **语言**：TypeScript
- **HTTP 框架**：Express
- **跨链 SDK**：@lifi/sdk
- **EVM Provider**：Viem（LiFi SDK 内置支持）
- **配置管理**：dotenv

**配置参数：**
- `LIFI_API_KEY`：LiFi API 密钥（可选，用于提高速率限制）
- `ARBITRUM_RPC_URL`：Arbitrum RPC 地址
- `ARBITRUM_CHAIN_ID`：链 ID（421614 = Sepolia, 42161 = Mainnet）
- `ARBITRUM_USDC_ADDRESS`：Arbitrum 上的 USDC 合约地址
- `TRANSIT_WALLET_PRIVATE_KEY`：中转钱包私钥（hex 格式）
- `DEFAULT_SLIPPAGE`：默认滑点（默认 0.03 = 3%）
- `PORT`：HTTP 服务端口（默认 8085）
- `NODE_ENV`：环境（development 或 production）
- `CORS_ALLOWED_ORIGINS`：允许的 CORS 源（逗号分隔，可选）

**安全考虑：**
1. **私钥管理**：私钥通过环境变量配置，不应硬编码
2. **速率限制**：实现合理的速率限制，避免超过 LiFi API 限制
3. **并发控制**：使用队列控制并发请求，避免资源竞争
4. **CORS 配置**：生产环境限制允许的源，防止未授权访问

---

## Relayer 服务设计

### 功能模块

#### 1. 事件监听模块

**EVM 事件监听**：
- 监听 EVM 链（Arbitrum）的 `StakeEvent` 事件
- 使用 Web3.js 或 Ethers.js 连接到 EVM RPC
- 解析事件日志获取 `StakeEventData`

**SVM 事件监听**：
- 监听 SVM 链（Solana/1024chain）的 `StakeEvent` 事件
- 使用 Solana Web3.js 连接到 SVM RPC
- 解析 Anchor 事件获取 `StakeEventData`

#### 2. 签名模块（双算法支持）

Relayer 需要同时支持两种签名算法，根据目标链选择：

**Ed25519 签名（用于提交到 SVM）**：
- 算法：Ed25519（Solana 原生）
- 数据格式：Borsh 序列化的 `StakeEventData`
- 签名库：`@noble/ed25519` 或 `tweetnacl`
- 密钥对：Solana Keypair (32 字节私钥)
- 输出：64 字节签名 + `Ed25519Program` 指令

**ECDSA 签名（用于提交到 EVM）**：
- 算法：ECDSA (secp256k1)（Ethereum 原生）
- 数据格式：JSON 序列化 → SHA-256 → EIP-191 格式
- 签名库：`ethers.js` 或 `web3.js`
- 密钥对：Ethereum 私钥 (32 字节)
- 输出：65 字节签名 (r, s, v)

#### 3. 数据转换模块

**SVM → EVM 转换**：
```typescript
// 1. 从 SVM 事件获取数据（Borsh 格式）
const svmEventData = parseAnchorEvent(event);

// 2. 转换为 EVM 格式
const evmEventData = {
  sourceContract: svmEventData.sourceContract.toBase58(), // 转换为字符串
  targetContract: evmEventData.targetContract, // EVM 地址
  chainId: svmEventData.sourceChainId.toString(),
  blockHeight: svmEventData.blockHeight.toString(),
  amount: svmEventData.amount.toString(),
  sender: svmEventData.sender,
  receiverAddress: svmEventData.receiverAddress,
  nonce: svmEventData.nonce.toString()
};

// 3. JSON 序列化
const jsonString = JSON.stringify(evmEventData);

// 4. 计算哈希（SHA-256 + EIP-191）
const sha256Hash = createHash('sha256').update(jsonString).digest();
const ethSignedHash = keccak256(
  Buffer.concat([
    Buffer.from('\x19Ethereum Signed Message:\n32'),
    sha256Hash
  ])
);

// 5. 使用 ECDSA 私钥签名
const signature = await ecdsaWallet.signMessage(ethSignedHash);
```

**EVM → SVM 转换**：
```typescript
// 1. 从 EVM 事件获取数据
const evmEvent = parseEthereumEvent(log);

// 2. 转换为 SVM 格式（构造 Anchor 类型）
const svmEventData: StakeEventData = {
  sourceContract: new PublicKey(evmEvent.sourceContract),
  targetContract: new PublicKey(svmEvent.targetContract),
  sourceChainId: new BN(evmEvent.chainId),
  targetChainId: new BN(evmEvent.targetChainId),
  blockHeight: new BN(evmEvent.blockHeight),
  amount: new BN(evmEvent.amount),
  receiverAddress: evmEvent.receiverAddress,
  nonce: new BN(evmEvent.nonce)
};

// 3. Borsh 序列化
const message = program.coder.types.encode("StakeEventData", svmEventData);

// 4. 使用 Ed25519 私钥签名
const signature = await ed25519.sign(message, keypair.secretKey.slice(0, 32));

// 5. 创建 Ed25519Program 验证指令
const ed25519Ix = Ed25519Program.createInstructionWithPublicKey({
  publicKey: keypair.publicKey.toBytes(),
  message: message,
  signature: signature
});
```

#### 4. 交易提交模块

**提交到 EVM**：
- 构造 `submitSignature` 交易
- 使用 Ethers.js 发送交易
- 处理 Gas 估算和失败重试
- 监控交易确认状态

**提交到 SVM**：
- 构造交易包含两个指令：
  1. `Ed25519Program` 验证指令
  2. `submitSignature` 业务指令
- 使用 Solana Web3.js 发送交易
- 处理计算单元和失败重试
- 监控交易确认状态

### Relayer 密钥管理

每个 Relayer 需要维护两对密钥：

**Ed25519 密钥对（用于 SVM）**：
```typescript
// Solana Keypair
const svmKeypair = Keypair.generate();
// 或从现有密钥恢复
const svmKeypair = Keypair.fromSecretKey(secretKey);
```

**ECDSA 密钥对（用于 EVM）**：
```typescript
// Ethereum Wallet
const evmWallet = new ethers.Wallet(privateKey);
// 或生成新钱包
const evmWallet = ethers.Wallet.createRandom();
```

### 配置要求

- 支持最多 18 个 relayer
- 每个 relayer 独立运行，维护双密钥对（Ed25519 + ECDSA）
- 阈值：`Math.ceil(relayer_count * 2 / 3)`
- 支持至少 100 个未完成的跨链请求
- 支持至少 1200 个签名缓存

---

## 安全考虑

### 多签钱包管理

**Squad 多签程序：**
- 程序地址：`SMPLecH534NA9acB4bMolv7X6RBpK4rjn3LkN1gZXYjy`
- 主要功能：创建多签账户、提案和投票、执行提案

**实现方式：**
- 所有管理接口使用 Squad 多签钱包作为 `admin`
- 合约层面只验证 `admin` 签名，不关心多签逻辑
- 多签投票在外部（Squad 程序）处理
- 管理接口包括：`initialize`, `configure_usdc`, `configure_peer`, `add_relayer`, `remove_relayer`, `add_liquidity`, `withdraw_liquidity`

**使用示例：**
```typescript
// 创建 Squad 多签账户
const squad = new Squad(provider);
const multisig = await squad.createMultisig({
  threshold: 2,  // 需要2个签名
  members: [admin1, admin2, admin3],
});

// 使用多签钱包作为 admin 初始化合约
await program.methods
  .initialize()
  .accounts({
    admin: multisig.publicKey,  // 多签钱包地址
    vault: vaultPda,  // PDA 金库地址
    // ...
  })
  .rpc();  // 需要多签成员签名（外部处理）
```

**注意事项：**
- 多签操作需要额外的 gas 费用
- 多签投票需要等待，但只影响管理操作，不影响业务操作（解锁使用 PDA）
- 需要确保 Squad 程序在目标链（1024chain）上可用
- 多签成员可以变更（外部处理）

### 防重放攻击

1. **Nonce 递增判断机制**：
   - 每个质押请求有唯一的 nonce（64 位无符号整数）
   - 新 nonce 必须大于 `last_nonce`（递增判断）
   - 如果 nonce <= `last_nonce`，视为重放攻击，拒绝处理
   - Nonce 溢出处理：当达到 `u64::MAX` 时，重置为 0
2. **区块高度验证**：事件中包含 block_height，可用于额外验证

### 多签阈值

- 阈值计算：`Math.ceil(relayer_count * 2 / 3)`
- 当签名数 >= 阈值时解锁（即 2/3 向上取整的 relayer 签名）
- 最多支持 18 个 relayer，阈值范围：2-12

### 权限控制

- 管理员权限：初始化、配置、relayer 管理
- Relayer 权限：只能提交签名
- 用户权限：只能调用质押接口

### 账户安全

- 使用 PDA 确保账户所有权
- 解锁后可以关闭 CrossChainRequest 账户回收租金
- Nonce 递增判断确保不会重放已处理的请求

### Event Data 一致性验证机制（关键安全修复）

**问题背景：**
在修复前，系统存在一个潜在的漏洞：如果第一个 relayer 提交错误的 `event_data`（例如错误的 amount 或 receiver），后续 relayer 可能会提交不同的 `event_data`，导致跨链请求参数不一致，最终可能按照错误的数据解锁代币。

**修复方案：**
系统实现了严格的 event_data 一致性验证机制，采用"第一个提交者决定"的设计原则：

1. **第一个 relayer 提交时**：
   - 将传入的 `event_data` 存储为本次跨链请求的"标准答案"
   - 所有字段（sourceContract, targetContract, sourceChainId, targetChainId, blockHeight, amount, receiverAddress, nonce）被永久固定

2. **后续 relayer 提交时**：
   - **强制一致性检查**：验证传入的 `event_data` 是否与已存储的 `event_data` 完全一致
   - 如果任何字段不匹配，直接拒绝并返回错误 "Invalid event data"
   - 这确保所有 relayer 必须对相同的事件数据签名

3. **解锁时**：
   - 使用存储的 `event_data`（第一个 relayer 提交的），而不是函数参数的 `event_data`
   - 这确保即使后续 relayer 传入的参数被修改，解锁操作仍使用最初存储的正确数据

**安全性保证：**

- ✅ **防止数据不一致**：所有 relayer 必须提交完全相同的 `event_data`
- ✅ **防止参数篡改**：即使恶意 relayer 尝试提交不同的参数，也会被一致性检查拒绝
- ✅ **签名验证双重保护**：签名验证确保每个 relayer 的签名匹配其提交的 `event_data`，一致性检查确保所有 relayer 提交相同的 `event_data`
- ✅ **第一个 relayer 保护**：如果第一个 relayer 恶意提交错误数据，正常 relayer 无法通过签名验证（他们的签名是对正确数据的签名），导致无法达到阈值

**攻击场景分析：**

| 攻击场景 | 防御机制 | 结果 |
|---------|---------|------|
| 第一个 relayer 提交错误的 `event_data` | 正常 relayer 的签名验证会失败（签名是对正确数据的签名） | ❌ 无法达到阈值，流程卡住，不会执行错误的解锁 |
| 后续 relayer 提交不同的 `event_data` | 一致性检查拒绝（与存储的数据不匹配） | ❌ 交易被拒绝 |
| 部分 relayer 被劫持（< 2/3） | 阈值机制（需要 > 2/3 签名）+ 签名验证 | ❌ 无法达到阈值 |
| 超过 2/3 的 relayer 被同时劫持 | 这是系统威胁模型假设的攻击场景 | ⚠️ 所有多重签名系统的固有风险 |

**实现位置：**
- **SVM 合约**：`submit_signature` 函数中的 `else` 分支（第 219-232 行）
- **EVM 合约**：`submitSignature` 函数中的 `else` 分支（第 312-327 行）
- **错误代码**：`InvalidEventData` (SVM) 和 `InvalidEventData()` (EVM)

---

## 性能考虑

### Solana 账户大小优化

- ReceiverState 主账户：~708 bytes（固定大小）
- CrossChainRequest PDA：~600+ bytes（固定大小，每个请求独立账户）
- 支持无限 nonce（每个 nonce 独立账户）
- 支持至少 100 个未完成的请求同时存在

### 查询优化

- `last_nonce` 使用单值存储，O(1) 查找
- Nonce 递增判断：O(1) 比较操作
- `signed_relayers` 使用 Vec 存储，O(n) 查找（n <= 18）

### 成本考虑

- 每个 CrossChainRequest PDA 账户需要租金（约 0.001 SOL）
- 解锁后可以关闭账户回收租金
- 建议定期清理已解锁的账户

---

## 扩展性

### 未来优化方向

1. **使用位图压缩**：如果 relayer 数量固定，可以使用位图跟踪签名状态
2. **批量清理**：定期批量关闭已解锁的 CrossChainRequest 账户
3. **Nonce 递增判断**：通过 `last_nonce` 实现 O(1) 的重放检查，无需存储历史记录

### 限制说明

- **Relayer 数量**：最多 18 个（由 ReceiverState.relayers Vec 大小决定）
- **Nonce 数量**：理论上无限（每个 nonce 使用独立 PDA）
- **未完成请求数量**：至少支持 100 个（每个请求独立 PDA 账户）
- **签名缓存容量**：至少 1200 个签名（100 个请求 × 18 个 relayer = 1800 个签名）
- **Nonce 类型**：64 位无符号整数（u64），溢出时重置为 0

---

## 变更记录

| 日期 | 变更内容 |
|------|----------|
| 2025-11-14 | 初始设计文档，采用 PDA 方案支持无限请求和 21 个 relayer |
| 2025-11-15 | 重构设计：支持 18 个 relayer；nonce 使用递增判断机制；统一初始化函数；支持 100+ 未完成请求和 1200+ 签名缓存 |
| 2025-11-15 | **多签钱包集成**：管理接口使用多签钱包（Squad），合约层面不关心多签逻辑；金库使用 PDA 支持自动转账；新增流动性管理接口（add_liquidity, withdraw_liquidity） |
| 2025-11-15 | 整合 Squad 多签文档内容到设计文档和 API 文档，删除独立的 squad_multisig.md 文档 |
| 2025-11-15 | **密码学算法设计**：明确 SVM 使用 Ed25519（原生），EVM 使用 ECDSA (secp256k1) + EIP-191（原生）；Relayer 负责格式转换；最大化性能和安全性 |

