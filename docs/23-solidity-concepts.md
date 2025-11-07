# Solidity核心概念速查

> 验收辅助文档  
> 用途: 帮助理解合约代码中的关键概念

---

## 💰 payable 和 msg.value

### payable关键字

**作用**: 标记函数可以接收ETH

**示例**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable
```

**含义**: 
- 用户调用时可以附带ETH
- 没有payable则无法接收ETH

### msg.value

**定义**: 用户随交易发送的ETH金额（wei单位）

**使用**: `contracts/evm/src/CoreContract.sol:119`

**转账流程**:
1. 用户调用：`contract.function{value: 0.001 ether}()`
2. EVM自动：用户余额 -0.001 ETH
3. EVM自动：合约余额 +0.001 ETH
4. 函数内：`msg.value` = 用户发送的金额

**无需显式transfer** - EVM内置机制！

---

## ⚠️ revert 错误处理

### revert关键字

**作用**: 终止交易，回滚所有状态变更

**示例**: `contracts/evm/src/CoreContract.sol:119`
```solidity
if (msg.value < messageFee) revert InsufficientFee();
```

**效果**:
- 交易立即失败
- 所有状态改变撤销
- 发送的ETH退回用户
- 抛出错误信息

**类似**: 其他语言的throw/panic

---

## 🔢 Guardian Set Index

### 不是单个Guardian的ID！

**定义**: `contracts/evm/src/CoreContract.sol:26`

**真正含义**: Guardian集合的版本号

**数据结构**:
```
guardianSets[0] = Set版本0（19个Guardian地址）
guardianSets[1] = Set版本1（升级后的19个地址）
guardianSetIndex = 当前使用的版本（0或1）
```

### 为什么需要版本？

**场景1: 密钥泄露**
- 需要替换某个Guardian
- 创建新Set，不影响旧VAA验证

**场景2: Guardian变更**
- 运营商更换
- 增加/减少Guardian数量

**场景3: 平滑升级**
- 旧Set继续验证旧VAA
- 新Set验证新VAA
- 无服务中断

### 升级流程

1. 创建新Guardian Set
2. 通过治理投票（需要13/19签名）
3. guardianSetIndex切换到新版本
4. 旧Set设置过期时间
5. 平滑过渡

**查看**: `contracts/evm/src/CoreContract.sol:21` (guardianSets映射)

---

## 🔐 mapping 数据结构

### mapping是什么？

**定义**: Solidity的哈希表/字典

**示例**:
```solidity
mapping(address => uint64) public sequences;
mapping(bytes32 => bool) public consumedVAAs;
mapping(uint32 => GuardianSet) public guardianSets;
```

### 工作原理

**类似**: 
- JavaScript的Map
- Python的dict
- Rust的HashMap

**特点**:
- 键值对存储
- O(1)查询
- 自动初始化（默认值）

**示例说明**:
```solidity
mapping(address => uint64) public sequences;

// 使用：
sequences[user_address] = 5;     // 设置
uint64 seq = sequences[user_address]; // 读取
```

**查看**: 
- sequences: `contracts/evm/src/CoreContract.sol:26`
- consumedVAAs: `contracts/evm/src/CoreContract.sol:23`
- guardianSets: `contracts/evm/src/CoreContract.sol:21`

---

## 🔒 防重放机制

### consumedVAAs映射

**定义**: `contracts/evm/src/CoreContract.sol:23`
```solidity
mapping(bytes32 => bool) public consumedVAAs;
```

**作用**: 记录已处理的VAA，防止重复提交

**工作流程**:

1. VAA到达
2. 计算VAA hash
3. 检查：`if (consumedVAAs[vaaHash]) revert`
4. 验证通过后：`consumedVAAs[vaaHash] = true`
5. 下次相同VAA到达：检查失败，revert

**查看实现**: `contracts/evm/src/CoreContract.sol:242`

### 为什么需要？

**攻击场景**: 
- 用户发送消息"转100 USDC"
- Guardian生成VAA
- 恶意Relayer多次提交同一个VAA
- 如果没有防重放：用户被扣100 USDC多次！

**防护机制**:
- VAA hash作为唯一标识
- 首次验证后标记为已消费
- 再次提交会revert

---

## 📝 emit 事件

### 事件是什么？

**定义**: `contracts/evm/src/CoreContract.sol:125-131`
```solidity
emit LogMessagePublished(...)
```

**作用**: 
- 记录到区块链日志
- 链下程序（Guardian）可监听
- 不占用存储空间（更便宜）

### Guardian如何监听？

**流程**:
1. 合约emit事件
2. 事件写入交易日志
3. Guardian通过WebSocket订阅
4. 实时接收事件通知

**查看**: `guardian/src/watcher/evm.rs:57-96`

### 与存储变量的区别

**事件(emit)**:
- 只能读，不能改
- 链下可访问
- 便宜（Gas低）

**存储变量**:
- 可读可写
- 链上永久存储
- 昂贵（Gas高）

---

## 🎯 modifier 修饰符

### whenNotPaused

**定义**: `contracts/evm/src/CoreContract.sol:95-98`

**作用**: 暂停机制

**使用**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable whenNotPaused
```

**含义**: 
- 函数执行前先检查`!paused`
- 如果paused=true，立即revert
- 紧急情况下可暂停合约

### onlyOwner

**定义**: `contracts/evm/src/CoreContract.sol:100-103`

**作用**: 权限控制

**使用**: `contracts/evm/src/CoreContract.sol:193, 202`

**含义**:
- 只有owner可以调用
- 其他人调用会revert
- 用于管理函数

---

## 📊 storage vs memory

### storage

**含义**: 永久存储在区块链

**示例**: `contracts/evm/src/CoreContract.sol:259`
```solidity
GuardianSet storage guardianSet = guardianSets[...];
```

**特点**:
- 状态变量
- 修改会写入链上
- Gas消耗高

### memory

**含义**: 临时内存，函数结束后销毁

**示例**: `contracts/evm/src/CoreContract.sol:116`
```solidity
bytes memory payload
```

**特点**:
- 函数参数
- 函数执行期间有效
- Gas消耗低

### 区别

| 类型 | 位置 | 持久性 | Gas |
|------|------|-------|-----|
| storage | 链上 | 永久 | 高 |
| memory | 内存 | 临时 | 低 |

---

## 🔢 uint类型

### 常见类型

```solidity
uint8   // 0-255
uint16  // 0-65535  
uint32  // 0-4294967295
uint64  // 更大
uint256 // 最大（默认uint）
```

### 为什么用不同大小？

**Gas优化**:
- 小数字用小类型
- 节省存储空间
- 降低Gas消耗

**示例**: `contracts/evm/src/CoreContract.sol`
- `uint8 consistencyLevel` - 只需0-255
- `uint32 guardianSetIndex` - 足够用
- `uint64 sequence` - 序列号
- `uint256 messageFee` - 可能很大的金额

---

## 📦 bytes类型

### bytes vs bytes32

**bytes**: 动态长度字节数组
- 用于任意长度数据
- Gas成本随长度增加

**bytes32**: 固定32字节
- 用于哈希、地址（填充）
- Gas成本固定

**示例**:
```solidity
bytes memory payload      // 可变长消息内容
bytes32 vaaHash          // 固定32字节哈希
```

---

## 🔗 address类型

### 地址转换

**20字节 → 32字节**:

```solidity
// Ethereum地址是20字节
address sender = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266

// 转为32字节（用于VAA）:
bytes32 emitterAddress = bytes32(uint256(uint160(sender)));
// 前12字节为0，后20字节是地址
```

**查看**: `guardian/src/watcher/evm.rs:102-103`

---

## 🎓 验收时的理解要点

### 核心概念清单

1. **payable + msg.value** = 接收ETH的标准方式
2. **revert** = 失败时回滚交易
3. **guardianSetIndex** = Guardian集合版本号（不是单个ID）
4. **mapping** = 链上哈希表
5. **consumedVAAs** = 防重放攻击
6. **emit** = 发出事件供Guardian监听
7. **modifier** = 函数前置检查（如权限、暂停）

### 验收时重点

✅ **不要误解**:
- guardianSetIndex不是某个Guardian
- msg.value不需要显式转账代码
- revert会自动退款

✅ **关注设计**:
- Guardian Set版本管理（允许升级）
- 防重放机制（consumedVAAs）
- 事件驱动架构（emit + Watcher）

---

**此文档帮助理解验收文档中的代码！**

配合使用：`docs/22-acceptance-guide.md`


> 验收辅助文档  
> 用途: 帮助理解合约代码中的关键概念

---

## 💰 payable 和 msg.value

### payable关键字

**作用**: 标记函数可以接收ETH

**示例**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable
```

**含义**: 
- 用户调用时可以附带ETH
- 没有payable则无法接收ETH

### msg.value

**定义**: 用户随交易发送的ETH金额（wei单位）

**使用**: `contracts/evm/src/CoreContract.sol:119`

**转账流程**:
1. 用户调用：`contract.function{value: 0.001 ether}()`
2. EVM自动：用户余额 -0.001 ETH
3. EVM自动：合约余额 +0.001 ETH
4. 函数内：`msg.value` = 用户发送的金额

**无需显式transfer** - EVM内置机制！

---

## ⚠️ revert 错误处理

### revert关键字

**作用**: 终止交易，回滚所有状态变更

**示例**: `contracts/evm/src/CoreContract.sol:119`
```solidity
if (msg.value < messageFee) revert InsufficientFee();
```

**效果**:
- 交易立即失败
- 所有状态改变撤销
- 发送的ETH退回用户
- 抛出错误信息

**类似**: 其他语言的throw/panic

---

## 🔢 Guardian Set Index

### 不是单个Guardian的ID！

**定义**: `contracts/evm/src/CoreContract.sol:26`

**真正含义**: Guardian集合的版本号

**数据结构**:
```
guardianSets[0] = Set版本0（19个Guardian地址）
guardianSets[1] = Set版本1（升级后的19个地址）
guardianSetIndex = 当前使用的版本（0或1）
```

### 为什么需要版本？

**场景1: 密钥泄露**
- 需要替换某个Guardian
- 创建新Set，不影响旧VAA验证

**场景2: Guardian变更**
- 运营商更换
- 增加/减少Guardian数量

**场景3: 平滑升级**
- 旧Set继续验证旧VAA
- 新Set验证新VAA
- 无服务中断

### 升级流程

1. 创建新Guardian Set
2. 通过治理投票（需要13/19签名）
3. guardianSetIndex切换到新版本
4. 旧Set设置过期时间
5. 平滑过渡

**查看**: `contracts/evm/src/CoreContract.sol:21` (guardianSets映射)

---

## 🔐 mapping 数据结构

### mapping是什么？

**定义**: Solidity的哈希表/字典

**示例**:
```solidity
mapping(address => uint64) public sequences;
mapping(bytes32 => bool) public consumedVAAs;
mapping(uint32 => GuardianSet) public guardianSets;
```

### 工作原理

**类似**: 
- JavaScript的Map
- Python的dict
- Rust的HashMap

**特点**:
- 键值对存储
- O(1)查询
- 自动初始化（默认值）

**示例说明**:
```solidity
mapping(address => uint64) public sequences;

// 使用：
sequences[user_address] = 5;     // 设置
uint64 seq = sequences[user_address]; // 读取
```

**查看**: 
- sequences: `contracts/evm/src/CoreContract.sol:26`
- consumedVAAs: `contracts/evm/src/CoreContract.sol:23`
- guardianSets: `contracts/evm/src/CoreContract.sol:21`

---

## 🔒 防重放机制

### consumedVAAs映射

**定义**: `contracts/evm/src/CoreContract.sol:23`
```solidity
mapping(bytes32 => bool) public consumedVAAs;
```

**作用**: 记录已处理的VAA，防止重复提交

**工作流程**:

1. VAA到达
2. 计算VAA hash
3. 检查：`if (consumedVAAs[vaaHash]) revert`
4. 验证通过后：`consumedVAAs[vaaHash] = true`
5. 下次相同VAA到达：检查失败，revert

**查看实现**: `contracts/evm/src/CoreContract.sol:242`

### 为什么需要？

**攻击场景**: 
- 用户发送消息"转100 USDC"
- Guardian生成VAA
- 恶意Relayer多次提交同一个VAA
- 如果没有防重放：用户被扣100 USDC多次！

**防护机制**:
- VAA hash作为唯一标识
- 首次验证后标记为已消费
- 再次提交会revert

---

## 📝 emit 事件

### 事件是什么？

**定义**: `contracts/evm/src/CoreContract.sol:125-131`
```solidity
emit LogMessagePublished(...)
```

**作用**: 
- 记录到区块链日志
- 链下程序（Guardian）可监听
- 不占用存储空间（更便宜）

### Guardian如何监听？

**流程**:
1. 合约emit事件
2. 事件写入交易日志
3. Guardian通过WebSocket订阅
4. 实时接收事件通知

**查看**: `guardian/src/watcher/evm.rs:57-96`

### 与存储变量的区别

**事件(emit)**:
- 只能读，不能改
- 链下可访问
- 便宜（Gas低）

**存储变量**:
- 可读可写
- 链上永久存储
- 昂贵（Gas高）

---

## 🎯 modifier 修饰符

### whenNotPaused

**定义**: `contracts/evm/src/CoreContract.sol:95-98`

**作用**: 暂停机制

**使用**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable whenNotPaused
```

**含义**: 
- 函数执行前先检查`!paused`
- 如果paused=true，立即revert
- 紧急情况下可暂停合约

### onlyOwner

**定义**: `contracts/evm/src/CoreContract.sol:100-103`

**作用**: 权限控制

**使用**: `contracts/evm/src/CoreContract.sol:193, 202`

**含义**:
- 只有owner可以调用
- 其他人调用会revert
- 用于管理函数

---

## 📊 storage vs memory

### storage

**含义**: 永久存储在区块链

**示例**: `contracts/evm/src/CoreContract.sol:259`
```solidity
GuardianSet storage guardianSet = guardianSets[...];
```

**特点**:
- 状态变量
- 修改会写入链上
- Gas消耗高

### memory

**含义**: 临时内存，函数结束后销毁

**示例**: `contracts/evm/src/CoreContract.sol:116`
```solidity
bytes memory payload
```

**特点**:
- 函数参数
- 函数执行期间有效
- Gas消耗低

### 区别

| 类型 | 位置 | 持久性 | Gas |
|------|------|-------|-----|
| storage | 链上 | 永久 | 高 |
| memory | 内存 | 临时 | 低 |

---

## 🔢 uint类型

### 常见类型

```solidity
uint8   // 0-255
uint16  // 0-65535  
uint32  // 0-4294967295
uint64  // 更大
uint256 // 最大（默认uint）
```

### 为什么用不同大小？

**Gas优化**:
- 小数字用小类型
- 节省存储空间
- 降低Gas消耗

**示例**: `contracts/evm/src/CoreContract.sol`
- `uint8 consistencyLevel` - 只需0-255
- `uint32 guardianSetIndex` - 足够用
- `uint64 sequence` - 序列号
- `uint256 messageFee` - 可能很大的金额

---

## 📦 bytes类型

### bytes vs bytes32

**bytes**: 动态长度字节数组
- 用于任意长度数据
- Gas成本随长度增加

**bytes32**: 固定32字节
- 用于哈希、地址（填充）
- Gas成本固定

**示例**:
```solidity
bytes memory payload      // 可变长消息内容
bytes32 vaaHash          // 固定32字节哈希
```

---

## 🔗 address类型

### 地址转换

**20字节 → 32字节**:

```solidity
// Ethereum地址是20字节
address sender = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266

// 转为32字节（用于VAA）:
bytes32 emitterAddress = bytes32(uint256(uint160(sender)));
// 前12字节为0，后20字节是地址
```

**查看**: `guardian/src/watcher/evm.rs:102-103`

---

## 🎓 验收时的理解要点

### 核心概念清单

1. **payable + msg.value** = 接收ETH的标准方式
2. **revert** = 失败时回滚交易
3. **guardianSetIndex** = Guardian集合版本号（不是单个ID）
4. **mapping** = 链上哈希表
5. **consumedVAAs** = 防重放攻击
6. **emit** = 发出事件供Guardian监听
7. **modifier** = 函数前置检查（如权限、暂停）

### 验收时重点

✅ **不要误解**:
- guardianSetIndex不是某个Guardian
- msg.value不需要显式转账代码
- revert会自动退款

✅ **关注设计**:
- Guardian Set版本管理（允许升级）
- 防重放机制（consumedVAAs）
- 事件驱动架构（emit + Watcher）

---

**此文档帮助理解验收文档中的代码！**

配合使用：`docs/22-acceptance-guide.md`


> 验收辅助文档  
> 用途: 帮助理解合约代码中的关键概念

---

## 💰 payable 和 msg.value

### payable关键字

**作用**: 标记函数可以接收ETH

**示例**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable
```

**含义**: 
- 用户调用时可以附带ETH
- 没有payable则无法接收ETH

### msg.value

**定义**: 用户随交易发送的ETH金额（wei单位）

**使用**: `contracts/evm/src/CoreContract.sol:119`

**转账流程**:
1. 用户调用：`contract.function{value: 0.001 ether}()`
2. EVM自动：用户余额 -0.001 ETH
3. EVM自动：合约余额 +0.001 ETH
4. 函数内：`msg.value` = 用户发送的金额

**无需显式transfer** - EVM内置机制！

---

## ⚠️ revert 错误处理

### revert关键字

**作用**: 终止交易，回滚所有状态变更

**示例**: `contracts/evm/src/CoreContract.sol:119`
```solidity
if (msg.value < messageFee) revert InsufficientFee();
```

**效果**:
- 交易立即失败
- 所有状态改变撤销
- 发送的ETH退回用户
- 抛出错误信息

**类似**: 其他语言的throw/panic

---

## 🔢 Guardian Set Index

### 不是单个Guardian的ID！

**定义**: `contracts/evm/src/CoreContract.sol:26`

**真正含义**: Guardian集合的版本号

**数据结构**:
```
guardianSets[0] = Set版本0（19个Guardian地址）
guardianSets[1] = Set版本1（升级后的19个地址）
guardianSetIndex = 当前使用的版本（0或1）
```

### 为什么需要版本？

**场景1: 密钥泄露**
- 需要替换某个Guardian
- 创建新Set，不影响旧VAA验证

**场景2: Guardian变更**
- 运营商更换
- 增加/减少Guardian数量

**场景3: 平滑升级**
- 旧Set继续验证旧VAA
- 新Set验证新VAA
- 无服务中断

### 升级流程

1. 创建新Guardian Set
2. 通过治理投票（需要13/19签名）
3. guardianSetIndex切换到新版本
4. 旧Set设置过期时间
5. 平滑过渡

**查看**: `contracts/evm/src/CoreContract.sol:21` (guardianSets映射)

---

## 🔐 mapping 数据结构

### mapping是什么？

**定义**: Solidity的哈希表/字典

**示例**:
```solidity
mapping(address => uint64) public sequences;
mapping(bytes32 => bool) public consumedVAAs;
mapping(uint32 => GuardianSet) public guardianSets;
```

### 工作原理

**类似**: 
- JavaScript的Map
- Python的dict
- Rust的HashMap

**特点**:
- 键值对存储
- O(1)查询
- 自动初始化（默认值）

**示例说明**:
```solidity
mapping(address => uint64) public sequences;

// 使用：
sequences[user_address] = 5;     // 设置
uint64 seq = sequences[user_address]; // 读取
```

**查看**: 
- sequences: `contracts/evm/src/CoreContract.sol:26`
- consumedVAAs: `contracts/evm/src/CoreContract.sol:23`
- guardianSets: `contracts/evm/src/CoreContract.sol:21`

---

## 🔒 防重放机制

### consumedVAAs映射

**定义**: `contracts/evm/src/CoreContract.sol:23`
```solidity
mapping(bytes32 => bool) public consumedVAAs;
```

**作用**: 记录已处理的VAA，防止重复提交

**工作流程**:

1. VAA到达
2. 计算VAA hash
3. 检查：`if (consumedVAAs[vaaHash]) revert`
4. 验证通过后：`consumedVAAs[vaaHash] = true`
5. 下次相同VAA到达：检查失败，revert

**查看实现**: `contracts/evm/src/CoreContract.sol:242`

### 为什么需要？

**攻击场景**: 
- 用户发送消息"转100 USDC"
- Guardian生成VAA
- 恶意Relayer多次提交同一个VAA
- 如果没有防重放：用户被扣100 USDC多次！

**防护机制**:
- VAA hash作为唯一标识
- 首次验证后标记为已消费
- 再次提交会revert

---

## 📝 emit 事件

### 事件是什么？

**定义**: `contracts/evm/src/CoreContract.sol:125-131`
```solidity
emit LogMessagePublished(...)
```

**作用**: 
- 记录到区块链日志
- 链下程序（Guardian）可监听
- 不占用存储空间（更便宜）

### Guardian如何监听？

**流程**:
1. 合约emit事件
2. 事件写入交易日志
3. Guardian通过WebSocket订阅
4. 实时接收事件通知

**查看**: `guardian/src/watcher/evm.rs:57-96`

### 与存储变量的区别

**事件(emit)**:
- 只能读，不能改
- 链下可访问
- 便宜（Gas低）

**存储变量**:
- 可读可写
- 链上永久存储
- 昂贵（Gas高）

---

## 🎯 modifier 修饰符

### whenNotPaused

**定义**: `contracts/evm/src/CoreContract.sol:95-98`

**作用**: 暂停机制

**使用**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable whenNotPaused
```

**含义**: 
- 函数执行前先检查`!paused`
- 如果paused=true，立即revert
- 紧急情况下可暂停合约

### onlyOwner

**定义**: `contracts/evm/src/CoreContract.sol:100-103`

**作用**: 权限控制

**使用**: `contracts/evm/src/CoreContract.sol:193, 202`

**含义**:
- 只有owner可以调用
- 其他人调用会revert
- 用于管理函数

---

## 📊 storage vs memory

### storage

**含义**: 永久存储在区块链

**示例**: `contracts/evm/src/CoreContract.sol:259`
```solidity
GuardianSet storage guardianSet = guardianSets[...];
```

**特点**:
- 状态变量
- 修改会写入链上
- Gas消耗高

### memory

**含义**: 临时内存，函数结束后销毁

**示例**: `contracts/evm/src/CoreContract.sol:116`
```solidity
bytes memory payload
```

**特点**:
- 函数参数
- 函数执行期间有效
- Gas消耗低

### 区别

| 类型 | 位置 | 持久性 | Gas |
|------|------|-------|-----|
| storage | 链上 | 永久 | 高 |
| memory | 内存 | 临时 | 低 |

---

## 🔢 uint类型

### 常见类型

```solidity
uint8   // 0-255
uint16  // 0-65535  
uint32  // 0-4294967295
uint64  // 更大
uint256 // 最大（默认uint）
```

### 为什么用不同大小？

**Gas优化**:
- 小数字用小类型
- 节省存储空间
- 降低Gas消耗

**示例**: `contracts/evm/src/CoreContract.sol`
- `uint8 consistencyLevel` - 只需0-255
- `uint32 guardianSetIndex` - 足够用
- `uint64 sequence` - 序列号
- `uint256 messageFee` - 可能很大的金额

---

## 📦 bytes类型

### bytes vs bytes32

**bytes**: 动态长度字节数组
- 用于任意长度数据
- Gas成本随长度增加

**bytes32**: 固定32字节
- 用于哈希、地址（填充）
- Gas成本固定

**示例**:
```solidity
bytes memory payload      // 可变长消息内容
bytes32 vaaHash          // 固定32字节哈希
```

---

## 🔗 address类型

### 地址转换

**20字节 → 32字节**:

```solidity
// Ethereum地址是20字节
address sender = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266

// 转为32字节（用于VAA）:
bytes32 emitterAddress = bytes32(uint256(uint160(sender)));
// 前12字节为0，后20字节是地址
```

**查看**: `guardian/src/watcher/evm.rs:102-103`

---

## 🎓 验收时的理解要点

### 核心概念清单

1. **payable + msg.value** = 接收ETH的标准方式
2. **revert** = 失败时回滚交易
3. **guardianSetIndex** = Guardian集合版本号（不是单个ID）
4. **mapping** = 链上哈希表
5. **consumedVAAs** = 防重放攻击
6. **emit** = 发出事件供Guardian监听
7. **modifier** = 函数前置检查（如权限、暂停）

### 验收时重点

✅ **不要误解**:
- guardianSetIndex不是某个Guardian
- msg.value不需要显式转账代码
- revert会自动退款

✅ **关注设计**:
- Guardian Set版本管理（允许升级）
- 防重放机制（consumedVAAs）
- 事件驱动架构（emit + Watcher）

---

**此文档帮助理解验收文档中的代码！**

配合使用：`docs/22-acceptance-guide.md`


> 验收辅助文档  
> 用途: 帮助理解合约代码中的关键概念

---

## 💰 payable 和 msg.value

### payable关键字

**作用**: 标记函数可以接收ETH

**示例**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable
```

**含义**: 
- 用户调用时可以附带ETH
- 没有payable则无法接收ETH

### msg.value

**定义**: 用户随交易发送的ETH金额（wei单位）

**使用**: `contracts/evm/src/CoreContract.sol:119`

**转账流程**:
1. 用户调用：`contract.function{value: 0.001 ether}()`
2. EVM自动：用户余额 -0.001 ETH
3. EVM自动：合约余额 +0.001 ETH
4. 函数内：`msg.value` = 用户发送的金额

**无需显式transfer** - EVM内置机制！

---

## ⚠️ revert 错误处理

### revert关键字

**作用**: 终止交易，回滚所有状态变更

**示例**: `contracts/evm/src/CoreContract.sol:119`
```solidity
if (msg.value < messageFee) revert InsufficientFee();
```

**效果**:
- 交易立即失败
- 所有状态改变撤销
- 发送的ETH退回用户
- 抛出错误信息

**类似**: 其他语言的throw/panic

---

## 🔢 Guardian Set Index

### 不是单个Guardian的ID！

**定义**: `contracts/evm/src/CoreContract.sol:26`

**真正含义**: Guardian集合的版本号

**数据结构**:
```
guardianSets[0] = Set版本0（19个Guardian地址）
guardianSets[1] = Set版本1（升级后的19个地址）
guardianSetIndex = 当前使用的版本（0或1）
```

### 为什么需要版本？

**场景1: 密钥泄露**
- 需要替换某个Guardian
- 创建新Set，不影响旧VAA验证

**场景2: Guardian变更**
- 运营商更换
- 增加/减少Guardian数量

**场景3: 平滑升级**
- 旧Set继续验证旧VAA
- 新Set验证新VAA
- 无服务中断

### 升级流程

1. 创建新Guardian Set
2. 通过治理投票（需要13/19签名）
3. guardianSetIndex切换到新版本
4. 旧Set设置过期时间
5. 平滑过渡

**查看**: `contracts/evm/src/CoreContract.sol:21` (guardianSets映射)

---

## 🔐 mapping 数据结构

### mapping是什么？

**定义**: Solidity的哈希表/字典

**示例**:
```solidity
mapping(address => uint64) public sequences;
mapping(bytes32 => bool) public consumedVAAs;
mapping(uint32 => GuardianSet) public guardianSets;
```

### 工作原理

**类似**: 
- JavaScript的Map
- Python的dict
- Rust的HashMap

**特点**:
- 键值对存储
- O(1)查询
- 自动初始化（默认值）

**示例说明**:
```solidity
mapping(address => uint64) public sequences;

// 使用：
sequences[user_address] = 5;     // 设置
uint64 seq = sequences[user_address]; // 读取
```

**查看**: 
- sequences: `contracts/evm/src/CoreContract.sol:26`
- consumedVAAs: `contracts/evm/src/CoreContract.sol:23`
- guardianSets: `contracts/evm/src/CoreContract.sol:21`

---

## 🔒 防重放机制

### consumedVAAs映射

**定义**: `contracts/evm/src/CoreContract.sol:23`
```solidity
mapping(bytes32 => bool) public consumedVAAs;
```

**作用**: 记录已处理的VAA，防止重复提交

**工作流程**:

1. VAA到达
2. 计算VAA hash
3. 检查：`if (consumedVAAs[vaaHash]) revert`
4. 验证通过后：`consumedVAAs[vaaHash] = true`
5. 下次相同VAA到达：检查失败，revert

**查看实现**: `contracts/evm/src/CoreContract.sol:242`

### 为什么需要？

**攻击场景**: 
- 用户发送消息"转100 USDC"
- Guardian生成VAA
- 恶意Relayer多次提交同一个VAA
- 如果没有防重放：用户被扣100 USDC多次！

**防护机制**:
- VAA hash作为唯一标识
- 首次验证后标记为已消费
- 再次提交会revert

---

## 📝 emit 事件

### 事件是什么？

**定义**: `contracts/evm/src/CoreContract.sol:125-131`
```solidity
emit LogMessagePublished(...)
```

**作用**: 
- 记录到区块链日志
- 链下程序（Guardian）可监听
- 不占用存储空间（更便宜）

### Guardian如何监听？

**流程**:
1. 合约emit事件
2. 事件写入交易日志
3. Guardian通过WebSocket订阅
4. 实时接收事件通知

**查看**: `guardian/src/watcher/evm.rs:57-96`

### 与存储变量的区别

**事件(emit)**:
- 只能读，不能改
- 链下可访问
- 便宜（Gas低）

**存储变量**:
- 可读可写
- 链上永久存储
- 昂贵（Gas高）

---

## 🎯 modifier 修饰符

### whenNotPaused

**定义**: `contracts/evm/src/CoreContract.sol:95-98`

**作用**: 暂停机制

**使用**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable whenNotPaused
```

**含义**: 
- 函数执行前先检查`!paused`
- 如果paused=true，立即revert
- 紧急情况下可暂停合约

### onlyOwner

**定义**: `contracts/evm/src/CoreContract.sol:100-103`

**作用**: 权限控制

**使用**: `contracts/evm/src/CoreContract.sol:193, 202`

**含义**:
- 只有owner可以调用
- 其他人调用会revert
- 用于管理函数

---

## 📊 storage vs memory

### storage

**含义**: 永久存储在区块链

**示例**: `contracts/evm/src/CoreContract.sol:259`
```solidity
GuardianSet storage guardianSet = guardianSets[...];
```

**特点**:
- 状态变量
- 修改会写入链上
- Gas消耗高

### memory

**含义**: 临时内存，函数结束后销毁

**示例**: `contracts/evm/src/CoreContract.sol:116`
```solidity
bytes memory payload
```

**特点**:
- 函数参数
- 函数执行期间有效
- Gas消耗低

### 区别

| 类型 | 位置 | 持久性 | Gas |
|------|------|-------|-----|
| storage | 链上 | 永久 | 高 |
| memory | 内存 | 临时 | 低 |

---

## 🔢 uint类型

### 常见类型

```solidity
uint8   // 0-255
uint16  // 0-65535  
uint32  // 0-4294967295
uint64  // 更大
uint256 // 最大（默认uint）
```

### 为什么用不同大小？

**Gas优化**:
- 小数字用小类型
- 节省存储空间
- 降低Gas消耗

**示例**: `contracts/evm/src/CoreContract.sol`
- `uint8 consistencyLevel` - 只需0-255
- `uint32 guardianSetIndex` - 足够用
- `uint64 sequence` - 序列号
- `uint256 messageFee` - 可能很大的金额

---

## 📦 bytes类型

### bytes vs bytes32

**bytes**: 动态长度字节数组
- 用于任意长度数据
- Gas成本随长度增加

**bytes32**: 固定32字节
- 用于哈希、地址（填充）
- Gas成本固定

**示例**:
```solidity
bytes memory payload      // 可变长消息内容
bytes32 vaaHash          // 固定32字节哈希
```

---

## 🔗 address类型

### 地址转换

**20字节 → 32字节**:

```solidity
// Ethereum地址是20字节
address sender = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266

// 转为32字节（用于VAA）:
bytes32 emitterAddress = bytes32(uint256(uint160(sender)));
// 前12字节为0，后20字节是地址
```

**查看**: `guardian/src/watcher/evm.rs:102-103`

---

## 🎓 验收时的理解要点

### 核心概念清单

1. **payable + msg.value** = 接收ETH的标准方式
2. **revert** = 失败时回滚交易
3. **guardianSetIndex** = Guardian集合版本号（不是单个ID）
4. **mapping** = 链上哈希表
5. **consumedVAAs** = 防重放攻击
6. **emit** = 发出事件供Guardian监听
7. **modifier** = 函数前置检查（如权限、暂停）

### 验收时重点

✅ **不要误解**:
- guardianSetIndex不是某个Guardian
- msg.value不需要显式转账代码
- revert会自动退款

✅ **关注设计**:
- Guardian Set版本管理（允许升级）
- 防重放机制（consumedVAAs）
- 事件驱动架构（emit + Watcher）

---

**此文档帮助理解验收文档中的代码！**

配合使用：`docs/22-acceptance-guide.md`


> 验收辅助文档  
> 用途: 帮助理解合约代码中的关键概念

---

## 💰 payable 和 msg.value

### payable关键字

**作用**: 标记函数可以接收ETH

**示例**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable
```

**含义**: 
- 用户调用时可以附带ETH
- 没有payable则无法接收ETH

### msg.value

**定义**: 用户随交易发送的ETH金额（wei单位）

**使用**: `contracts/evm/src/CoreContract.sol:119`

**转账流程**:
1. 用户调用：`contract.function{value: 0.001 ether}()`
2. EVM自动：用户余额 -0.001 ETH
3. EVM自动：合约余额 +0.001 ETH
4. 函数内：`msg.value` = 用户发送的金额

**无需显式transfer** - EVM内置机制！

---

## ⚠️ revert 错误处理

### revert关键字

**作用**: 终止交易，回滚所有状态变更

**示例**: `contracts/evm/src/CoreContract.sol:119`
```solidity
if (msg.value < messageFee) revert InsufficientFee();
```

**效果**:
- 交易立即失败
- 所有状态改变撤销
- 发送的ETH退回用户
- 抛出错误信息

**类似**: 其他语言的throw/panic

---

## 🔢 Guardian Set Index

### 不是单个Guardian的ID！

**定义**: `contracts/evm/src/CoreContract.sol:26`

**真正含义**: Guardian集合的版本号

**数据结构**:
```
guardianSets[0] = Set版本0（19个Guardian地址）
guardianSets[1] = Set版本1（升级后的19个地址）
guardianSetIndex = 当前使用的版本（0或1）
```

### 为什么需要版本？

**场景1: 密钥泄露**
- 需要替换某个Guardian
- 创建新Set，不影响旧VAA验证

**场景2: Guardian变更**
- 运营商更换
- 增加/减少Guardian数量

**场景3: 平滑升级**
- 旧Set继续验证旧VAA
- 新Set验证新VAA
- 无服务中断

### 升级流程

1. 创建新Guardian Set
2. 通过治理投票（需要13/19签名）
3. guardianSetIndex切换到新版本
4. 旧Set设置过期时间
5. 平滑过渡

**查看**: `contracts/evm/src/CoreContract.sol:21` (guardianSets映射)

---

## 🔐 mapping 数据结构

### mapping是什么？

**定义**: Solidity的哈希表/字典

**示例**:
```solidity
mapping(address => uint64) public sequences;
mapping(bytes32 => bool) public consumedVAAs;
mapping(uint32 => GuardianSet) public guardianSets;
```

### 工作原理

**类似**: 
- JavaScript的Map
- Python的dict
- Rust的HashMap

**特点**:
- 键值对存储
- O(1)查询
- 自动初始化（默认值）

**示例说明**:
```solidity
mapping(address => uint64) public sequences;

// 使用：
sequences[user_address] = 5;     // 设置
uint64 seq = sequences[user_address]; // 读取
```

**查看**: 
- sequences: `contracts/evm/src/CoreContract.sol:26`
- consumedVAAs: `contracts/evm/src/CoreContract.sol:23`
- guardianSets: `contracts/evm/src/CoreContract.sol:21`

---

## 🔒 防重放机制

### consumedVAAs映射

**定义**: `contracts/evm/src/CoreContract.sol:23`
```solidity
mapping(bytes32 => bool) public consumedVAAs;
```

**作用**: 记录已处理的VAA，防止重复提交

**工作流程**:

1. VAA到达
2. 计算VAA hash
3. 检查：`if (consumedVAAs[vaaHash]) revert`
4. 验证通过后：`consumedVAAs[vaaHash] = true`
5. 下次相同VAA到达：检查失败，revert

**查看实现**: `contracts/evm/src/CoreContract.sol:242`

### 为什么需要？

**攻击场景**: 
- 用户发送消息"转100 USDC"
- Guardian生成VAA
- 恶意Relayer多次提交同一个VAA
- 如果没有防重放：用户被扣100 USDC多次！

**防护机制**:
- VAA hash作为唯一标识
- 首次验证后标记为已消费
- 再次提交会revert

---

## 📝 emit 事件

### 事件是什么？

**定义**: `contracts/evm/src/CoreContract.sol:125-131`
```solidity
emit LogMessagePublished(...)
```

**作用**: 
- 记录到区块链日志
- 链下程序（Guardian）可监听
- 不占用存储空间（更便宜）

### Guardian如何监听？

**流程**:
1. 合约emit事件
2. 事件写入交易日志
3. Guardian通过WebSocket订阅
4. 实时接收事件通知

**查看**: `guardian/src/watcher/evm.rs:57-96`

### 与存储变量的区别

**事件(emit)**:
- 只能读，不能改
- 链下可访问
- 便宜（Gas低）

**存储变量**:
- 可读可写
- 链上永久存储
- 昂贵（Gas高）

---

## 🎯 modifier 修饰符

### whenNotPaused

**定义**: `contracts/evm/src/CoreContract.sol:95-98`

**作用**: 暂停机制

**使用**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable whenNotPaused
```

**含义**: 
- 函数执行前先检查`!paused`
- 如果paused=true，立即revert
- 紧急情况下可暂停合约

### onlyOwner

**定义**: `contracts/evm/src/CoreContract.sol:100-103`

**作用**: 权限控制

**使用**: `contracts/evm/src/CoreContract.sol:193, 202`

**含义**:
- 只有owner可以调用
- 其他人调用会revert
- 用于管理函数

---

## 📊 storage vs memory

### storage

**含义**: 永久存储在区块链

**示例**: `contracts/evm/src/CoreContract.sol:259`
```solidity
GuardianSet storage guardianSet = guardianSets[...];
```

**特点**:
- 状态变量
- 修改会写入链上
- Gas消耗高

### memory

**含义**: 临时内存，函数结束后销毁

**示例**: `contracts/evm/src/CoreContract.sol:116`
```solidity
bytes memory payload
```

**特点**:
- 函数参数
- 函数执行期间有效
- Gas消耗低

### 区别

| 类型 | 位置 | 持久性 | Gas |
|------|------|-------|-----|
| storage | 链上 | 永久 | 高 |
| memory | 内存 | 临时 | 低 |

---

## 🔢 uint类型

### 常见类型

```solidity
uint8   // 0-255
uint16  // 0-65535  
uint32  // 0-4294967295
uint64  // 更大
uint256 // 最大（默认uint）
```

### 为什么用不同大小？

**Gas优化**:
- 小数字用小类型
- 节省存储空间
- 降低Gas消耗

**示例**: `contracts/evm/src/CoreContract.sol`
- `uint8 consistencyLevel` - 只需0-255
- `uint32 guardianSetIndex` - 足够用
- `uint64 sequence` - 序列号
- `uint256 messageFee` - 可能很大的金额

---

## 📦 bytes类型

### bytes vs bytes32

**bytes**: 动态长度字节数组
- 用于任意长度数据
- Gas成本随长度增加

**bytes32**: 固定32字节
- 用于哈希、地址（填充）
- Gas成本固定

**示例**:
```solidity
bytes memory payload      // 可变长消息内容
bytes32 vaaHash          // 固定32字节哈希
```

---

## 🔗 address类型

### 地址转换

**20字节 → 32字节**:

```solidity
// Ethereum地址是20字节
address sender = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266

// 转为32字节（用于VAA）:
bytes32 emitterAddress = bytes32(uint256(uint160(sender)));
// 前12字节为0，后20字节是地址
```

**查看**: `guardian/src/watcher/evm.rs:102-103`

---

## 🎓 验收时的理解要点

### 核心概念清单

1. **payable + msg.value** = 接收ETH的标准方式
2. **revert** = 失败时回滚交易
3. **guardianSetIndex** = Guardian集合版本号（不是单个ID）
4. **mapping** = 链上哈希表
5. **consumedVAAs** = 防重放攻击
6. **emit** = 发出事件供Guardian监听
7. **modifier** = 函数前置检查（如权限、暂停）

### 验收时重点

✅ **不要误解**:
- guardianSetIndex不是某个Guardian
- msg.value不需要显式转账代码
- revert会自动退款

✅ **关注设计**:
- Guardian Set版本管理（允许升级）
- 防重放机制（consumedVAAs）
- 事件驱动架构（emit + Watcher）

---

**此文档帮助理解验收文档中的代码！**

配合使用：`docs/22-acceptance-guide.md`


> 验收辅助文档  
> 用途: 帮助理解合约代码中的关键概念

---

## 💰 payable 和 msg.value

### payable关键字

**作用**: 标记函数可以接收ETH

**示例**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable
```

**含义**: 
- 用户调用时可以附带ETH
- 没有payable则无法接收ETH

### msg.value

**定义**: 用户随交易发送的ETH金额（wei单位）

**使用**: `contracts/evm/src/CoreContract.sol:119`

**转账流程**:
1. 用户调用：`contract.function{value: 0.001 ether}()`
2. EVM自动：用户余额 -0.001 ETH
3. EVM自动：合约余额 +0.001 ETH
4. 函数内：`msg.value` = 用户发送的金额

**无需显式transfer** - EVM内置机制！

---

## ⚠️ revert 错误处理

### revert关键字

**作用**: 终止交易，回滚所有状态变更

**示例**: `contracts/evm/src/CoreContract.sol:119`
```solidity
if (msg.value < messageFee) revert InsufficientFee();
```

**效果**:
- 交易立即失败
- 所有状态改变撤销
- 发送的ETH退回用户
- 抛出错误信息

**类似**: 其他语言的throw/panic

---

## 🔢 Guardian Set Index

### 不是单个Guardian的ID！

**定义**: `contracts/evm/src/CoreContract.sol:26`

**真正含义**: Guardian集合的版本号

**数据结构**:
```
guardianSets[0] = Set版本0（19个Guardian地址）
guardianSets[1] = Set版本1（升级后的19个地址）
guardianSetIndex = 当前使用的版本（0或1）
```

### 为什么需要版本？

**场景1: 密钥泄露**
- 需要替换某个Guardian
- 创建新Set，不影响旧VAA验证

**场景2: Guardian变更**
- 运营商更换
- 增加/减少Guardian数量

**场景3: 平滑升级**
- 旧Set继续验证旧VAA
- 新Set验证新VAA
- 无服务中断

### 升级流程

1. 创建新Guardian Set
2. 通过治理投票（需要13/19签名）
3. guardianSetIndex切换到新版本
4. 旧Set设置过期时间
5. 平滑过渡

**查看**: `contracts/evm/src/CoreContract.sol:21` (guardianSets映射)

---

## 🔐 mapping 数据结构

### mapping是什么？

**定义**: Solidity的哈希表/字典

**示例**:
```solidity
mapping(address => uint64) public sequences;
mapping(bytes32 => bool) public consumedVAAs;
mapping(uint32 => GuardianSet) public guardianSets;
```

### 工作原理

**类似**: 
- JavaScript的Map
- Python的dict
- Rust的HashMap

**特点**:
- 键值对存储
- O(1)查询
- 自动初始化（默认值）

**示例说明**:
```solidity
mapping(address => uint64) public sequences;

// 使用：
sequences[user_address] = 5;     // 设置
uint64 seq = sequences[user_address]; // 读取
```

**查看**: 
- sequences: `contracts/evm/src/CoreContract.sol:26`
- consumedVAAs: `contracts/evm/src/CoreContract.sol:23`
- guardianSets: `contracts/evm/src/CoreContract.sol:21`

---

## 🔒 防重放机制

### consumedVAAs映射

**定义**: `contracts/evm/src/CoreContract.sol:23`
```solidity
mapping(bytes32 => bool) public consumedVAAs;
```

**作用**: 记录已处理的VAA，防止重复提交

**工作流程**:

1. VAA到达
2. 计算VAA hash
3. 检查：`if (consumedVAAs[vaaHash]) revert`
4. 验证通过后：`consumedVAAs[vaaHash] = true`
5. 下次相同VAA到达：检查失败，revert

**查看实现**: `contracts/evm/src/CoreContract.sol:242`

### 为什么需要？

**攻击场景**: 
- 用户发送消息"转100 USDC"
- Guardian生成VAA
- 恶意Relayer多次提交同一个VAA
- 如果没有防重放：用户被扣100 USDC多次！

**防护机制**:
- VAA hash作为唯一标识
- 首次验证后标记为已消费
- 再次提交会revert

---

## 📝 emit 事件

### 事件是什么？

**定义**: `contracts/evm/src/CoreContract.sol:125-131`
```solidity
emit LogMessagePublished(...)
```

**作用**: 
- 记录到区块链日志
- 链下程序（Guardian）可监听
- 不占用存储空间（更便宜）

### Guardian如何监听？

**流程**:
1. 合约emit事件
2. 事件写入交易日志
3. Guardian通过WebSocket订阅
4. 实时接收事件通知

**查看**: `guardian/src/watcher/evm.rs:57-96`

### 与存储变量的区别

**事件(emit)**:
- 只能读，不能改
- 链下可访问
- 便宜（Gas低）

**存储变量**:
- 可读可写
- 链上永久存储
- 昂贵（Gas高）

---

## 🎯 modifier 修饰符

### whenNotPaused

**定义**: `contracts/evm/src/CoreContract.sol:95-98`

**作用**: 暂停机制

**使用**: `contracts/evm/src/CoreContract.sol:118`
```solidity
function publishMessage(...) external payable whenNotPaused
```

**含义**: 
- 函数执行前先检查`!paused`
- 如果paused=true，立即revert
- 紧急情况下可暂停合约

### onlyOwner

**定义**: `contracts/evm/src/CoreContract.sol:100-103`

**作用**: 权限控制

**使用**: `contracts/evm/src/CoreContract.sol:193, 202`

**含义**:
- 只有owner可以调用
- 其他人调用会revert
- 用于管理函数

---

## 📊 storage vs memory

### storage

**含义**: 永久存储在区块链

**示例**: `contracts/evm/src/CoreContract.sol:259`
```solidity
GuardianSet storage guardianSet = guardianSets[...];
```

**特点**:
- 状态变量
- 修改会写入链上
- Gas消耗高

### memory

**含义**: 临时内存，函数结束后销毁

**示例**: `contracts/evm/src/CoreContract.sol:116`
```solidity
bytes memory payload
```

**特点**:
- 函数参数
- 函数执行期间有效
- Gas消耗低

### 区别

| 类型 | 位置 | 持久性 | Gas |
|------|------|-------|-----|
| storage | 链上 | 永久 | 高 |
| memory | 内存 | 临时 | 低 |

---

## 🔢 uint类型

### 常见类型

```solidity
uint8   // 0-255
uint16  // 0-65535  
uint32  // 0-4294967295
uint64  // 更大
uint256 // 最大（默认uint）
```

### 为什么用不同大小？

**Gas优化**:
- 小数字用小类型
- 节省存储空间
- 降低Gas消耗

**示例**: `contracts/evm/src/CoreContract.sol`
- `uint8 consistencyLevel` - 只需0-255
- `uint32 guardianSetIndex` - 足够用
- `uint64 sequence` - 序列号
- `uint256 messageFee` - 可能很大的金额

---

## 📦 bytes类型

### bytes vs bytes32

**bytes**: 动态长度字节数组
- 用于任意长度数据
- Gas成本随长度增加

**bytes32**: 固定32字节
- 用于哈希、地址（填充）
- Gas成本固定

**示例**:
```solidity
bytes memory payload      // 可变长消息内容
bytes32 vaaHash          // 固定32字节哈希
```

---

## 🔗 address类型

### 地址转换

**20字节 → 32字节**:

```solidity
// Ethereum地址是20字节
address sender = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266

// 转为32字节（用于VAA）:
bytes32 emitterAddress = bytes32(uint256(uint160(sender)));
// 前12字节为0，后20字节是地址
```

**查看**: `guardian/src/watcher/evm.rs:102-103`

---

## 🎓 验收时的理解要点

### 核心概念清单

1. **payable + msg.value** = 接收ETH的标准方式
2. **revert** = 失败时回滚交易
3. **guardianSetIndex** = Guardian集合版本号（不是单个ID）
4. **mapping** = 链上哈希表
5. **consumedVAAs** = 防重放攻击
6. **emit** = 发出事件供Guardian监听
7. **modifier** = 函数前置检查（如权限、暂停）

### 验收时重点

✅ **不要误解**:
- guardianSetIndex不是某个Guardian
- msg.value不需要显式转账代码
- revert会自动退款

✅ **关注设计**:
- Guardian Set版本管理（允许升级）
- 防重放机制（consumedVAAs）
- 事件驱动架构（emit + Watcher）

---

**此文档帮助理解验收文档中的代码！**

配合使用：`docs/22-acceptance-guide.md`

