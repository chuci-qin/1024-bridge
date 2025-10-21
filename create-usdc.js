const { Connection, Keypair, PublicKey, Transaction, sendAndConfirmTransaction, SystemProgram } = require('@solana/web3.js');
const { createInitializeMintInstruction, TOKEN_PROGRAM_ID, MINT_SIZE, getMinimumBalanceForRentExemptMint } = require('@solana/spl-token');
const fs = require('fs');

// 1024Chain Testnet的Token Program ID
const CUSTOM_TOKEN_PROGRAM = new PublicKey('BTNcjvKBL5A2p5an7hB2SY8whcJSKxDv5uv5viM8hYvg');

async function createUSDC() {
    console.log('🪙 创建USDC Token...');
    
    // 连接到1024Chain testnet
    const connection = new Connection('https://testnet-rpc.1024chain.com/rpc/', 'confirmed');
    
    // 读取钱包（使用solana config中配置的钱包）
    const walletPath = '/tmp/alice_test.json';  // 从solana config get获取
    const walletData = JSON.parse(fs.readFileSync(walletPath, 'utf8'));
    const payer = Keypair.fromSecretKey(new Uint8Array(walletData));
    
    console.log('👛 钱包地址:', payer.publicKey.toString());
    
    // 生成USDC Mint
    const mintKeypair = Keypair.generate();
    
    console.log('🆔 USDC Mint地址:', mintKeypair.publicKey.toString());
    
    // 获取租金
    const lamports = await getMinimumBalanceForRentExemptMint(connection);
    
    // 创建账户指令
    const createAccountIx = SystemProgram.createAccount({
        fromPubkey: payer.publicKey,
        newAccountPubkey: mintKeypair.publicKey,
        space: MINT_SIZE,
        lamports,
        programId: CUSTOM_TOKEN_PROGRAM,
    });
    
    // 初始化Mint指令
    const initializeMintIx = createInitializeMintInstruction(
        mintKeypair.publicKey,  // mint
        6,                       // decimals (USDC是6位)
        payer.publicKey,         // mint authority
        payer.publicKey,         // freeze authority
        CUSTOM_TOKEN_PROGRAM     // 使用自定义Token Program
    );
    
    // 发送交易
    const tx = new Transaction().add(createAccountIx, initializeMintIx);
    
    console.log('📤 发送交易...');
    const signature = await sendAndConfirmTransaction(
        connection,
        tx,
        [payer, mintKeypair]
    );
    
    console.log('✅ USDC创建成功!');
    console.log('   Mint地址:', mintKeypair.publicKey.toString());
    console.log('   交易签名:', signature);
    console.log('');
    console.log('💾 保存Mint地址到.usdc-mint文件');
    fs.writeFileSync('.usdc-mint', mintKeypair.publicKey.toString());
    
    return mintKeypair.publicKey.toString();
}

createUSDC()
    .then(() => process.exit(0))
    .catch((err) => {
        console.error('❌ 错误:', err);
        process.exit(1);
    });
