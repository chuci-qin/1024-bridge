use anyhow::{anyhow, Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::post,
    Router,
};
use ethers::{
    abi::Abi,
    contract::Contract,
    core::types::Address,
    middleware::SignerMiddleware,
    prelude::*,
    providers::{Http, Provider},
    signers::{LocalWallet, Signer as EthersSigner},
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DepositProof {
    /// 源链 ID（例如 Ethereum Mainnet / Polygon 等）
    source_chain_id: u64,
    /// 源链或目标链上的交易哈希（当前主要使用 Arbitrum 最终 txHash）
    source_tx_hash: String,
    /// 源链代币地址
    source_token_address: String,
    /// 源链代币金额（最小单位，十进制字符串）
    source_amount: String,
    /// 目标链 ID（Arbitrum = 42161）
    target_chain_id: u64,
    /// 目标代币地址（Arbitrum USDC 合约地址）
    target_token_address: String,
    /// 目标 USDC 金额（最小单位，十进制字符串）
    target_amount: String,
    /// 用户地址（EVM 地址，0x...）
    from_address: String,
    /// Broker 中转钱包地址（Arbitrum 地址，0x...）
    to_address: String,
    /// 1024chain 接收地址（字符串）
    target1024_address: String,
    /// 第一段跨链完成的时间戳（秒）
    timestamp: u64,
    /// 可选：LiFi routeId
    #[serde(default)]
    lifi_route_id: Option<String>,
    /// 用户对证明内容的签名（EIP-191）
    user_signature: String,
}

#[derive(Debug, Deserialize)]
struct StakeRequest {
    /// USDC 金额（字符串格式，最小单位）
    amount: String,
    /// 1024chain 接收地址
    target_address: String,
    /// Deposit 方向的交易证明（包含 LiFi / Arbitrum 的交易信息）
    proof: DepositProof,
}

#[derive(Debug, Serialize)]
struct StakeResponse {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tx_hash: Option<String>,
}

#[derive(Clone)]
struct AppState {
    bridge_contract: Arc<Contract<SignerMiddleware<Provider<Http>, LocalWallet>>>,
    usdc_contract: Arc<Contract<SignerMiddleware<Provider<Http>, LocalWallet>>>,
    wallet_address: Address,
    // 使用 Mutex 序列化交易发送，避免 nonce 冲突和余额检查竞态
    tx_mutex: Arc<Mutex<()>>,
}

/// 简单的进程内防重放缓存
///
/// 说明：
/// - 当前实现针对**单实例部署**是安全的（进程内 Mutex + HashSet）
/// - TODO: 如果在生产中采用多实例 / 多进程部署，必须将其替换为：
///   - Redis：使用 `SET key value NX EX ttl` 实现原子占用
///   - 或数据库：使用唯一键约束 + 事务插入
/// 否则不同实例之间无法共享消费状态，会存在重复消费同一个 proofId 的风险。
static CONSUMED_PROOFS: Lazy<StdMutex<HashSet<String>>> = Lazy::new(|| StdMutex::new(HashSet::new()));

fn build_proof_id(proof: &DepositProof) -> String {
    // 使用链ID + 交易哈希作为唯一ID
    format!("{}:{}", proof.source_chain_id, proof.source_tx_hash)
}

/// 尝试占用 proofId，返回 true 表示首次占用，false 表示已被使用
fn try_consume_proof(proof_id: &str) -> bool {
    let mut set = CONSUMED_PROOFS
        .lock()
        .expect("proof cache mutex poisoned");
    if set.contains(proof_id) {
        return false;
    }
    set.insert(proof_id.to_string());
    true
}

// Bridge 合约 ABI（仅包含需要的函数）
const BRIDGE_ABI: &str = r#"
[
    {
        "inputs": [
            {"internalType": "uint256", "name": "amount", "type": "uint256"},
            {"internalType": "string", "name": "receiverAddress", "type": "string"}
        ],
        "name": "stake",
        "outputs": [{"internalType": "uint64", "name": "", "type": "uint64"}],
        "stateMutability": "nonpayable",
        "type": "function"
    }
]
"#;

// ERC20 USDC 合约 ABI（仅包含需要的函数）
const ERC20_ABI: &str = r#"
[
    {
        "inputs": [
            {"internalType": "address", "name": "spender", "type": "address"},
            {"internalType": "uint256", "name": "amount", "type": "uint256"}
        ],
        "name": "approve",
        "outputs": [{"internalType": "bool", "name": "", "type": "bool"}],
        "stateMutability": "nonpayable",
        "type": "function"
    },
    {
        "inputs": [
            {"internalType": "address", "name": "owner", "type": "address"},
            {"internalType": "address", "name": "spender", "type": "address"}
        ],
        "name": "allowance",
        "outputs": [{"internalType": "uint256", "name": "", "type": "uint256"}],
        "stateMutability": "view",
        "type": "function"
    },
    {
        "inputs": [{"internalType": "address", "name": "account", "type": "address"}],
        "name": "balanceOf",
        "outputs": [{"internalType": "uint256", "name": "", "type": "uint256"}],
        "stateMutability": "view",
        "type": "function"
    }
]
"#;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    // 默认日志级别：RUST_LOG=info
    // 可以通过环境变量设置：RUST_LOG=debug 查看详细调试信息
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        )
        .init();

    // 加载配置
    dotenvy::dotenv().ok();

    let rpc_url = std::env::var("RPC_URL")
        .context("RPC_URL environment variable not set")?;

    let private_key_hex = std::env::var("PRIVATE_KEY")
        .context("PRIVATE_KEY environment variable not set (hex format, with or without 0x prefix)")?;

    let bridge_contract_address = std::env::var("BRIDGE_CONTRACT_ADDRESS")
        .context("BRIDGE_CONTRACT_ADDRESS environment variable not set")?;

    let usdc_contract_address = std::env::var("USDC_CONTRACT_ADDRESS")
        .context("USDC_CONTRACT_ADDRESS environment variable not set")?;

    let chain_id = std::env::var("CHAIN_ID")
        .unwrap_or_else(|_| "421614".to_string()) // 默认 Arbitrum Sepolia
        .parse::<u64>()
        .context("Invalid CHAIN_ID")?;

    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .context("Invalid PORT")?;

    // CORS 配置：从环境变量读取允许的源，默认允许 localhost:3000
    let allowed_origin = std::env::var("CORS_ALLOW_ORIGIN")
        .unwrap_or_else(|_| "http://localhost:3000".to_string());

    // 创建 Provider
    let provider = Provider::<Http>::try_from(rpc_url.clone())
        .context("Failed to create provider")?;

    // 解析私钥
    let private_key_hex = private_key_hex.strip_prefix("0x").unwrap_or(&private_key_hex);
    let wallet: LocalWallet = private_key_hex
        .parse()
        .context("Failed to parse private key")?;

    // 设置链 ID
    let wallet = wallet.with_chain_id(chain_id);

    // 创建签名中间件
    let client = Arc::new(SignerMiddleware::new(provider, wallet.clone()));

    let wallet_address = wallet.address();

    // 解析合约地址
    let bridge_address: Address = bridge_contract_address
        .parse()
        .context("Invalid BRIDGE_CONTRACT_ADDRESS")?;

    let usdc_address: Address = usdc_contract_address
        .parse()
        .context("Invalid USDC_CONTRACT_ADDRESS")?;

    // 解析 ABI 并创建合约实例
    let bridge_abi: Abi = serde_json::from_str(BRIDGE_ABI)
        .context("Failed to parse BRIDGE_ABI")?;

    let usdc_abi: Abi = serde_json::from_str(ERC20_ABI)
        .context("Failed to parse ERC20_ABI")?;

    let bridge_contract = Arc::new(Contract::new(bridge_address, bridge_abi, client.clone()));
    let usdc_contract = Arc::new(Contract::new(usdc_address, usdc_abi, client.clone()));

    info!(
        rpc_url = %rpc_url,
        wallet_address = %wallet_address,
        bridge_contract = %bridge_address,
        usdc_contract = %usdc_address,
        chain_id = chain_id,
        port = port,
        "EVM Gateway service starting"
    );

    // 创建应用状态
    let state = AppState {
        bridge_contract,
        usdc_contract,
        wallet_address,
        tx_mutex: Arc::new(Mutex::new(())),
    };

    // 配置 CORS
    // 注意：当 allow_credentials(true) 时，不能使用 Any 作为 allow_headers
    // 必须明确指定允许的请求头
    let cors = CorsLayer::new()
        .allow_origin(allowed_origin.parse::<axum::http::HeaderValue>().unwrap())
        .allow_methods([axum::http::Method::GET, axum::http::Method::POST, axum::http::Method::OPTIONS])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ])
        .allow_credentials(true);
    
    info!(
        cors_origin = %allowed_origin,
        "CORS configured"
    );

    // 创建路由
    let app = Router::new()
        .route("/stake", post(handle_stake))
        .layer(cors)
        .with_state(state);

    // 启动服务器
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .context("Failed to bind to address")?;

    info!(port = port, "EVM Gateway service listening");

    axum::serve(listener, app)
        .await
        .context("Server error")?;

    Ok(())
}

async fn handle_stake(
    State(state): State<AppState>,
    Json(req): Json<StakeRequest>,
) -> Result<Json<StakeResponse>, (StatusCode, Json<StakeResponse>)> {
    info!(
        target_address = %req.target_address,
        amount = %req.amount,
        "Received stake request"
    );

    // 先验证交易证明（本地校验 + 防重放占用）
    if let Err(e) = verify_deposit_proof(&state, &req).await {
        error!(
            error = %e,
            "Deposit proof verification failed"
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(StakeResponse {
                success: false,
                message: format!("Proof verification failed: {}", e),
                tx_hash: None,
            }),
        ));
    }

    match stake_to_1024chain(&state, &req.amount, &req.target_address).await {
        Ok(tx_hash) => {
            info!(
                tx_hash = %tx_hash,
                amount = %req.amount,
                target_address = %req.target_address,
                "Stake request completed successfully"
            );
            Ok(Json(StakeResponse {
                success: true,
                message: "Stake successful".to_string(),
                tx_hash: Some(tx_hash),
            }))
        }
        Err(e) => {
            error!(
                error = %e,
                amount = %req.amount,
                target_address = %req.target_address,
                "Stake request failed"
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(StakeResponse {
                    success: false,
                    message: format!("Stake failed: {}", e),
                    tx_hash: None,
                }),
            ))
        }
    }
}

/// 验证 Deposit 方向的交易证明
///
/// 注意：当前实现只做本地一致性校验与进程内防重放，后续可以扩展：
/// - 通过 Arbitrum RPC 验证 source_tx_hash 对应的交易状态、确认数、金额等
/// - 使用 Redis / 数据库做跨进程 / 跨实例的原子防重放
async fn verify_deposit_proof(_state: &AppState, req: &StakeRequest) -> Result<()> {
    let proof = &req.proof;

    // 1. 基本字段校验
    if req.amount.trim().is_empty() {
        return Err(anyhow!("amount is empty"));
    }
    if proof.target_amount.trim().is_empty() {
        return Err(anyhow!("proof.targetAmount is empty"));
    }

    // 2. 金额一致性（请求 amount 与 proof.targetAmount 应一致）
    if req.amount != proof.target_amount {
        return Err(anyhow!(
            "amount mismatch: request={}, proof={}",
            req.amount,
            proof.target_amount
        ));
    }

    // 3. 目标地址一致性（请求 target_address 与 proof.target1024Address 应一致）
    if req.target_address != proof.target1024_address {
        return Err(anyhow!(
            "target address mismatch: request={}, proof={}",
            req.target_address,
            proof.target1024_address
        ));
    }

    // 4. 简单的时间戳检查（防止明显过期的证明）
    // 这里只做格式性检查，具体时间窗口策略可在后续实现中完善
    if proof.timestamp == 0 {
        return Err(anyhow!("invalid timestamp in proof"));
    }

    // 5. 原子防重放：尝试占用 proofId
    let proof_id = build_proof_id(proof);
    if !try_consume_proof(&proof_id) {
        return Err(anyhow!("proof already used"));
    }

    // TODO(security):
    // - 使用 state 中的 provider 查询 Arbitrum 上 source_tx_hash 对应的交易与 receipt
    // - 验证交易成功、确认数 >= MIN_CONFIRMATIONS[42161]
    // - 验证 ERC20 Transfer 日志中 to == proof.toAddress、amount ≈ proof.targetAmount
    // - 验证 proof.userSignature（EIP-191）恢复的地址与 proof.fromAddress 一致

    Ok(())
}

async fn stake_to_1024chain(
    state: &AppState,
    amount_str: &str,
    receiver_address: &str,
) -> Result<String> {
    // 解析金额（支持字符串格式的大数）
    // 注意：U256::from_dec_str 用于解析十进制字符串，避免被误解析为十六进制
    let amount: U256 = U256::from_dec_str(amount_str)
        .context("Failed to parse amount as decimal")?;

    info!(
        amount_str = %amount_str,
        amount_parsed = %amount,
        "Parsed amount from string"
    );

    // 验证 amount 不超过 uint64::MAX，因为事件中会转换为 uint64
    // uint64::MAX = 18,446,744,073,709,551,615
    const U64_MAX: u64 = u64::MAX;
    if amount > U256::from(U64_MAX) {
        return Err(anyhow::anyhow!(
            "Amount {} exceeds uint64::MAX ({})",
            amount,
            U64_MAX
        ));
    }

    // 使用 Mutex 序列化关键操作，避免并发问题：
    // 1. 余额检查竞态条件
    // 2. Nonce 冲突
    // 3. Approve 竞态条件
    let _guard = state.tx_mutex.lock().await;

    // 1. 检查 USDC 余额
    let balance: U256 = state
        .usdc_contract
        .method::<_, U256>("balanceOf", state.wallet_address)?
        .call()
        .await
        .context("Failed to check USDC balance")?;

    info!(
        balance = %balance,
        required_amount = %amount,
        "Checking USDC balance"
    );

    if balance < amount {
        error!(
            balance = %balance,
            required = %amount,
            "Insufficient USDC balance"
        );
        return Err(anyhow::anyhow!(
            "Insufficient USDC balance: have {}, need {}",
            balance,
            amount
        ));
    }

    // 2. 检查并授权 USDC
    let bridge_address = state.bridge_contract.address();
    let allowance: U256 = state
        .usdc_contract
        .method::<_, U256>("allowance", (state.wallet_address, bridge_address))?
        .call()
        .await
        .context("Failed to check USDC allowance")?;


    // 使用一个超大的数额进行 approve，避免频繁 approve 操作
    // U256::MAX 可能导致某些合约拒绝，使用一个足够大的固定值
    // 例如：10^12 USDC（假设 6 位小数，即 1,000,000 USDC）
    let max_approval_amount = U256::from(10_u64.pow(18)); // 10^18，足够大

    if allowance < amount {
        info!(
            allowance = %allowance,
            required = %amount,
            "Approving USDC - allowance insufficient"
        );
        let approve_method = state
            .usdc_contract
            .method::<_, bool>("approve", (bridge_address, max_approval_amount))
            .context("Failed to create approve method")?;
        
        let approve_tx = approve_method
            .send()
            .await
            .context("Failed to send approve transaction")?;

        let approve_receipt = approve_tx
            .await?
            .context("Failed to get approve receipt")?;

        info!(
            approve_tx_hash = %approve_receipt.transaction_hash,
            "USDC approved"
        );
    }

    // 3. 调用 stake 函数
    let receiver_addr = receiver_address.to_string();
    
    info!(
        amount_before_call = %amount,
        receiver = %receiver_addr,
        "Calling stake method"
    );
    
    let method = state
        .bridge_contract
        .method::<_, u64>("stake", (amount, receiver_addr))
        .context("Failed to create stake method")?;
    
    let pending_tx = method
        .send()
        .await
        .context("Failed to send stake transaction")?;

    let stake_receipt = pending_tx
        .await?
        .context("Failed to get stake receipt")?;

    let tx_hash = format!("{:?}", stake_receipt.transaction_hash);

    // 验证事件中的amount是否正确（通过解析receipt中的事件）
    // 注意：这里我们只是记录，实际的验证由relayer完成
    info!(
        tx_hash = %tx_hash,
        amount = %amount,
        amount_u64 = %amount.as_u64(),
        receiver = %receiver_address,
        "Stake transaction confirmed"
    );

    Ok(tx_hash)
}
