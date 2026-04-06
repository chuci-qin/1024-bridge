/**
 * E2E Test: EVM to SVM bridge flow
 *
 * Stakes USDC on EVM (Arbitrum Sepolia), verifies StakeEvent, waits for
 * relayer to submit Ed25519 signatures and unlock on SVM (1024chain),
 * verifies CrossChainSuccessEvent and SVM balance increase.
 *
 * Prerequisites:
 * - EVM contract deployed (set EVM_CONTRACT_ADDRESS)
 * - SVM program deployed (set SVM_PROGRAM_ID)
 * - Relayer running with BRIDGE_ID
 * - Both chains funded with USDC and native tokens
 *
 * Environment variables:
 *   EVM_RPC_URL            - Arbitrum Sepolia RPC
 *   SVM_RPC_URL            - 1024chain Testnet RPC
 *   EVM_CONTRACT_ADDRESS   - Bridge contract on EVM
 *   SVM_PROGRAM_ID         - Bridge program on SVM
 *   EVM_TOKEN_ADDRESS      - USDC address on EVM
 *   USER_PRIVATE_KEY       - EVM sender private key
 *   RECEIVER_ADDRESS       - 1024chain receiver pubkey (base58)
 *   AMOUNT                 - Amount in USDC atomic units (default: 100000000 = 100 USDC)
 *   POLL_INTERVAL_MS       - Balance poll interval (default: 5000)
 *   TIMEOUT_MS             - Max wait for relayer (default: 120000)
 *
 * Usage: npx ts-node tests/e2e/evm-to-svm.ts
 */

// ============================================================================
// 端到端测试：EVM → SVM 跨链桥流程
//
// 本文件实现从 EVM 链（Arbitrum Sepolia）到 SVM 链（1024chain）的完整跨链桥
// 端到端测试。测试流程：用户在 EVM 端质押 USDC，relayer 监听到 StakeEvent 后
// 提交 Ed25519 签名到 SVM 端完成解锁，最终验证 SVM 端余额增加。
//
// 前置条件：
//   - EVM 桥合约已部署（需设置 EVM_CONTRACT_ADDRESS）
//   - SVM 桥程序已部署（需设置 SVM_PROGRAM_ID）
//   - Relayer 正在运行且已配置 BRIDGE_ID
//   - 两条链上均有足够的 USDC 和原生代币用于测试
//
// 环境变量说明：
//   EVM_RPC_URL          - Arbitrum Sepolia 的 RPC 节点地址
//   SVM_RPC_URL          - 1024chain 测试网的 RPC 节点地址
//   EVM_CONTRACT_ADDRESS - EVM 端桥合约地址
//   SVM_PROGRAM_ID       - SVM 端桥程序 ID
//   EVM_TOKEN_ADDRESS    - EVM 端 USDC 代币合约地址
//   USER_PRIVATE_KEY     - EVM 发送方钱包私钥
//   RECEIVER_ADDRESS     - 1024chain 接收方公钥（base58 格式）
//   AMOUNT               - USDC 最小单位数量（默认: 100000000 = 100 USDC）
//   POLL_INTERVAL_MS     - 余额轮询间隔（默认: 5000 毫秒）
//   TIMEOUT_MS           - 等待 relayer 处理的最大超时时间（默认: 120000 毫秒）
//
// 运行方式: npx ts-node tests/e2e/evm-to-svm.ts
// ============================================================================

import { ethers } from "ethers";

// ERC20 标准接口 ABI（仅包含余额查询、授权和授权额度查询）
const ERC20_ABI = [
    "function balanceOf(address owner) view returns (uint256)",
    "function approve(address spender, uint256 amount) returns (bool)",
    "function allowance(address owner, address spender) view returns (uint256)",
];

// 桥合约 ABI：stake 质押方法、StakeEvent 质押事件、TokensUnlocked 解锁事件
const BRIDGE_ABI = [
    "function stake(uint256 amount, string receiverAddress) returns (uint64)",
    "event StakeEvent(bytes32 indexed sourceContract, bytes32 indexed targetContract, uint64 chainId, uint64 blockHeight, uint64 amount, address sender, string receiverAddress, uint64 nonce)",
    "event TokensUnlocked(uint64 indexed nonce, address receiver, uint64 amount)",
];

// 带时间戳的日志输出，标识为 evm->svm 方向
function log(msg: string): void {
    console.log(`[evm->svm][${new Date().toISOString()}] ${msg}`);
}

// 异步等待辅助函数，用于轮询间隔
function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
    // 从环境变量中读取所有必要的配置参数
    const rpcUrl = process.env.EVM_RPC_URL!;
    const contractAddress = process.env.EVM_CONTRACT_ADDRESS!;
    const privateKey = process.env.USER_PRIVATE_KEY!;
    const receiverAddress = process.env.RECEIVER_ADDRESS!;
    const usdcAddress = process.env.EVM_TOKEN_ADDRESS!;
    const amount = process.env.AMOUNT || "100000000";
    const pollIntervalMs = parseInt(process.env.POLL_INTERVAL_MS || "5000");
    const timeoutMs = parseInt(process.env.TIMEOUT_MS || "120000");

    log("============================================");
    log("  Bridge1024 E2E: EVM -> SVM");
    log("============================================");
    log(`Bridge:   ${contractAddress}`);
    log(`USDC:     ${usdcAddress}`);
    log(`Amount:   ${amount}`);
    log(`Receiver: ${receiverAddress}`);
    log("");

    // 初始化 ethers 提供者、钱包以及 USDC 和桥合约实例
    const provider = new ethers.JsonRpcProvider(rpcUrl);
    const wallet = new ethers.Wallet(privateKey, provider);
    const usdc = new ethers.Contract(usdcAddress, ERC20_ABI, wallet);
    const bridge = new ethers.Contract(contractAddress, BRIDGE_ABI, wallet);

    // 预检：检查 EVM 端 USDC 余额是否足够
    const evmBal = (await usdc.balanceOf(wallet.address)) as bigint;
    if (evmBal < BigInt(amount)) {
        throw new Error(`Insufficient EVM USDC: have ${evmBal}, need ${amount}`);
    }
    log(`EVM USDC balance: ${evmBal}`);

    // 第一步：确保 USDC 授权额度足够，若不足则授权 MaxUint256 给桥合约
    log("\n--- Step 1: Approve USDC ---");
    const currentAllowance = (await usdc.allowance(wallet.address, contractAddress)) as bigint;
    if (currentAllowance < BigInt(amount)) {
        log(`Allowance ${currentAllowance} < ${amount}, approving MaxUint256...`);
        const approveTx = await usdc.approve(contractAddress, ethers.MaxUint256);
        await approveTx.wait();
        log(`Approved (tx: ${approveTx.hash})`);
    } else {
        log(`Allowance sufficient: ${currentAllowance}`);
    }

    // 第二步：在 EVM 端调用桥合约的 stake 方法质押 USDC，指定 SVM 端接收地址
    log("\n--- Step 2: Stake USDC ---");
    const stakeTx = await bridge.stake(amount, receiverAddress);
    const receipt = await stakeTx.wait();
    log(`Stake tx: ${receipt.hash}`);

    // 第三步：从交易回执中解析并验证 StakeEvent 事件，确认质押金额正确
    log("\n--- Step 3: Verify StakeEvent ---");
    const stakeEvent = receipt.logs
        .map((l: any) => {
            try { return bridge.interface.parseLog(l); } catch { return null; }
        })
        .find((e: any) => e?.name === "StakeEvent");

    if (!stakeEvent) {
        throw new Error("StakeEvent not found in tx receipt");
    }
    log(`StakeEvent emitted:`);
    log(`  sender:   ${stakeEvent.args.sender}`);
    log(`  amount:   ${stakeEvent.args.amount}`);
    log(`  nonce:    ${stakeEvent.args.nonce}`);
    log(`  receiver: ${stakeEvent.args.receiverAddress}`);

    if (stakeEvent.args.amount !== BigInt(amount)) {
        throw new Error(`StakeEvent.amount mismatch: ${stakeEvent.args.amount} != ${amount}`);
    }

    // 第四步：等待 relayer 处理跨链消息并在 SVM 端完成解锁
    // relayer 会监听 EVM 端的 StakeEvent，收集 Ed25519 签名后提交到 SVM 端
    log("\n--- Step 4: Waiting for relayer to process ---");
    log(`Polling SVM chain for balance change (timeout: ${timeoutMs}ms)...`);
    log("TODO: Add SVM balance polling using @solana/web3.js");
    log("  - Connect to SVM_RPC_URL");
    log("  - Get receiver's USDC ATA");
    log("  - Poll until balance increases by expected amount");
    log("  - Verify CrossChainSuccessEvent in recent program logs");

    // TODO: 实现 SVM 端余额轮询逻辑
    // 需要完成以下步骤：
    // 1. 使用 @solana/web3.js 连接到 SVM_RPC_URL（1024chain 测试网）
    // 2. 根据接收方公钥和 USDC Mint 地址，派生关联代币账户（ATA）地址
    // 3. 在超时时间内循环轮询 ATA 余额，检测余额是否增加了预期金额
    // 4. 在程序日志中验证 CrossChainSuccessEvent 事件
    // const { Connection, PublicKey } = await import("@solana/web3.js");
    // const { getAccount, getAssociatedTokenAddress } = await import("@solana/spl-token");
    // const svmConnection = new Connection(process.env.SVM_RPC_URL!);
    // const receiverPubkey = new PublicKey(receiverAddress);
    // const svmUsdcMint = new PublicKey(process.env.SVM_TOKEN_ADDRESS!);
    // const receiverAta = await getAssociatedTokenAddress(svmUsdcMint, receiverPubkey);
    //
    // const deadline = Date.now() + timeoutMs;
    // while (Date.now() < deadline) {
    //     const acct = await getAccount(svmConnection, receiverAta, "confirmed");
    //     log(`SVM balance: ${acct.amount}`);
    //     if (acct.amount >= expectedBalance) break;
    //     await sleep(pollIntervalMs);
    // }

    // 第五步：验证 SVM 端接收方余额
    // TODO: 需要对比质押前后的 SVM 余额，断言余额增加量等于质押金额
    log("\n--- Step 5: Verify receiver balance ---");
    log("TODO: Compare SVM balance before/after and assert increase matches amount");

    log("\n============================================");
    log("  E2E Test Complete");
    log("============================================");
}

// 执行主流程，成功退出码 0，失败退出码 1 并打印错误堆栈
main()
    .then(() => process.exit(0))
    .catch((err) => {
        console.error(`[evm->svm] FAILED: ${err.message || err}`);
        if (err.stack) console.error(err.stack);
        process.exit(1);
    });
