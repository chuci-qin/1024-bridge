# 数字签名原理速查 (结合 VAA)

> 5 分钟理解数字签名在跨链桥中的应用

---

## 1. 核心概念

### 1.1 什么是数字签名?

**类比**: 现实世界的手写签名 + 防伪水印

```
消息 + 私钥 → 签名 (任何人都能验证,但只有私钥持有者能创建)
```

**三个关键属性**:
1. ✅ **真实性** - 证明消息确实由私钥持有者发出
2. ✅ **完整性** - 消息内容未被篡改
3. ✅ **不可否认** - 签名者无法否认曾签署过此消息

---

## 2. ECDSA 签名流程 (VAA 实际使用)

### 2.1 密钥对生成

```rust
// Guardian 启动时生成
let secp = Secp256k1::new();
let (secret_key, public_key) = secp.generate_keypair(&mut rng);

// 从公钥推导以太坊地址 (用于 VAA 验证)
let pubkey_bytes = public_key.serialize_uncompressed();
let hash = keccak256(&pubkey_bytes[1..]); // 去掉第一个字节(0x04)
let eth_address = &hash[12..]; // 取后 20 字节
```

**数学基础**: 椭圆曲线 secp256k1
- **私钥**: 256 位随机数 (必须保密 🔒)
- **公钥**: 从私钥通过椭圆曲线运算得出 (可公开 📢)
- **单向性**: 从公钥无法反推私钥 (数学难题)

### 2.2 签名过程

```rust
// Guardian 收到消息后签名
pub fn sign_vaa(vaa_body: &VAABody, secret_key: &SecretKey) -> Signature {
    // 1. 计算消息摘要 (双重哈希)
    let body_bytes = vaa_body.serialize();
    let hash1 = keccak256(&body_bytes);
    let digest = keccak256(&hash1);  // 32 字节摘要
    
    // 2. 用私钥对摘要签名
    let secp = Secp256k1::new();
    let message = Message::from_digest(digest);
    let (recovery_id, signature) = secp.sign_ecdsa_recoverable(&message, secret_key);
    
    // 3. 拆分签名为 (r, s, v)
    let (r, s) = signature.serialize_compact();
    let v = recovery_id.to_i32() as u8 + 27;  // 27 或 28
    
    Signature { r, s, v }
}
```

**输出**: 65 字节签名
- `r`: 32 字节 (签名的 x 坐标)
- `s`: 32 字节 (签名参数)
- `v`: 1 字节 (恢复 ID, 用于从签名恢复公钥)

### 2.3 验证过程

```solidity
// 目标链智能合约验证 VAA
function verifyVAA(bytes memory vaa) public view returns (bool) {
    // 1. 解析 VAA 结构
    (VAABody memory body, Signature[] memory sigs) = parseVAA(vaa);
    
    // 2. 重新计算消息摘要
    bytes memory bodyBytes = serializeBody(body);
    bytes32 hash1 = keccak256(bodyBytes);
    bytes32 digest = keccak256(abi.encodePacked(hash1));
    
    // 3. 逐个验证签名
    uint validSigs = 0;
    for (uint i = 0; i < sigs.length; i++) {
        // 从签名恢复公钥对应的以太坊地址
        address signer = ecrecover(digest, sigs[i].v, sigs[i].r, sigs[i].s);
        
        // 检查签名者是否在 Guardian Set 中
        if (isGuardian(signer, body.guardianSetIndex)) {
            validSigs++;
        }
    }
    
    // 4. 检查是否达到法定人数 (13/19)
    return validSigs >= quorum;
}
```

---

## 3. VAA 中的多签机制

### 3.1 数据结构

```
┌─────────────────────────────────────────────┐
│              VAA (完整签名消息)                │
├─────────────────────────────────────────────┤
│ Header                                      │
│  ├─ version: 1                              │
│  ├─ guardianSetIndex: 0                     │
│  └─ signaturesCount: 13                     │
├─────────────────────────────────────────────┤
│ Signatures (13/19 个)                       │
│  ├─ Guardian[3]: (r, s, v)  ← 签名 1        │
│  ├─ Guardian[7]: (r, s, v)  ← 签名 2        │
│  ├─ Guardian[9]: (r, s, v)  ← 签名 3        │
│  └─ ...                                     │
├─────────────────────────────────────────────┤
│ Body (所有 Guardian 签署的内容)              │
│  ├─ timestamp: 1699264800                   │
│  ├─ nonce: 12345                            │
│  ├─ emitterChain: 1 (Ethereum)              │
│  ├─ emitterAddress: 0xABC...                │
│  ├─ sequence: 42                            │
│  ├─ consistencyLevel: 200                   │
│  └─ payload: 0x... (实际跨链数据)            │
└─────────────────────────────────────────────┘
```

### 3.2 为什么需要多签?

**单签问题**:
```
Guardian-1 签名 → 用户提交 → 目标链验证 ✅
    ↓ 如果 Guardian-1 作恶?
    🚨 可以签署虚假消息!
```

**多签方案** (13/19):
```
19 个 Guardian 独立观察事件
    ↓
至少 13 个签名一致
    ↓
拜占庭容错: 可容忍 6 个节点作恶/离线
    ↓
消息才被认为有效
```

**数学保证**:
- 需要攻击 `≥13` 个节点才能伪造消息
- 概率极低 (假设节点独立运营)

---

## 4. 实战示例: 跨链转账

### 场景: Alice 从 Ethereum 转 100 USDC 到 Solana

```
第 1 步: Ethereum 合约发出事件
─────────────────────────────────────
LogMessagePublished {
    sender: 0xAlice,
    sequence: 100,
    payload: "Transfer 100 USDC to Solana:Bob"
}

第 2 步: 19 个 Guardian 独立监听
─────────────────────────────────────
Guardian-1: ✅ 看到事件 → 签名 "我确认序列号 100 消息"
Guardian-2: ✅ 看到事件 → 签名 "我确认序列号 100 消息"
...
Guardian-13: ✅ 看到事件 → 签名 "我确认序列号 100 消息"
(总共 13+ 个签名)

第 3 步: 聚合 VAA
─────────────────────────────────────
VAA = {
    signatures: [sig1, sig2, ..., sig13],
    body: {
        emitterChain: 1,
        sequence: 100,
        payload: "Transfer 100 USDC to Solana:Bob"
    }
}

第 4 步: 用户提交到 Solana
─────────────────────────────────────
Solana 程序验证:
1. 重新计算消息摘要 ✅
2. 验证 13 个签名有效 ✅
3. 检查签名者在 Guardian Set ✅
4. 检查序列号未重放 ✅
→ 铸造 100 USDC 给 Bob
```

---

## 5. 关键安全点

### 5.1 防重放攻击

```rust
// 每个消息都有唯一标识
pub struct MessageID {
    emitter_chain: u16,
    emitter_address: [u8; 32],
    sequence: u64,  // 递增序列号
}

// 合约记录已处理的消息
mapping(bytes32 => bool) public consumedMessages;

function consumeVAA(bytes memory vaa) public {
    bytes32 hash = keccak256(abi.encodePacked(
        vaa.emitterChain,
        vaa.emitterAddress,
        vaa.sequence
    ));
    
    require(!consumedMessages[hash], "Message already consumed");
    consumedMessages[hash] = true;
    
    // 执行业务逻辑...
}
```

### 5.2 Guardian Set 更新

```solidity
// 当前 Guardian Set
struct GuardianSet {
    address[] keys;         // 19 个以太坊地址
    uint32 expirationTime;  // 过期时间
}

mapping(uint32 => GuardianSet) public guardianSets;
uint32 public currentGuardianSetIndex;

// 通过治理 VAA 更新
function updateGuardianSet(bytes memory governanceVAA) public {
    // 1. 验证 VAA 由当前 Guardian Set 签署
    // 2. 解析新的 Guardian 公钥列表
    // 3. 更新到下一个索引
    // 4. 设置旧 Set 的过期时间
}
```

---

## 6. 快速记忆要点

| 概念 | 一句话解释 | VAA 应用 |
|------|-----------|---------|
| **私钥** | 256 位秘密数字 | Guardian 用来签名 |
| **公钥** | 从私钥推导的点 | 推导以太坊地址 |
| **签名** | `sign(hash(message), 私钥)` | 每个 Guardian 对 VAA Body 签名 |
| **验证** | `ecrecover(hash, 签名) == 公钥地址` | 合约验证 13/19 签名有效 |
| **摘要** | `keccak256(keccak256(data))` | 双重哈希防止长度扩展攻击 |
| **多签** | M of N 签名才有效 | 13/19 拜占庭容错 |

---

## 7. 常见误区澄清

❌ **错误理解 1**: "签名就是加密"
- ✅ **正确**: 签名不加密内容,只证明来源和完整性
- VAA 内容是明文,任何人都能读取

❌ **错误理解 2**: "公钥可以解密签名得到消息"
- ✅ **正确**: 签名验证是数学验证,不是解密
- 验证公式: `G^k == R + hash(m) * PubKey` (椭圆曲线运算)

❌ **错误理解 3**: "有了签名就能修改消息"
- ✅ **正确**: 修改消息会导致摘要变化,签名立即失效
- 完整性保护: `verify(msg', sig) = false`

---

## 8. 延伸阅读

**数学原理**:
- ECDSA: 椭圆曲线数字签名算法
- secp256k1: 比特币/以太坊使用的曲线参数
- 恢复签名: 从 (r, s, v) 恢复公钥的数学技巧

**工程实践**:
- 确定性签名: RFC 6979 (防止随机数泄露私钥)
- 签名延展性: EIP-2 (规范化 s 值)
- 批量验证: 多个签名并行验证优化

**Wormhole 特定**:
- Guardian Set 治理机制
- VAA 格式规范
- 跨链消息协议

---

*最后更新: 2025-11-06*
