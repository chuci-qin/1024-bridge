//! EVM 确认交易提交模块
//!
//! 负责：
//! 1. 检查某个 nonce 是否已在 EVM 链上被确认（调用 nonceConfirmations 视图函数）
//! 2. 提交 confirmEvent 交易（带 StakeEventData 参数）
//!
//! ABI 编码说明：
//! - EVM 的 uint64 在 ABI 中占一个 32 字节 word，值大端序右对齐
//! - bytes32 直接占一个 32 字节 word

use anyhow::{Context, Result};
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, TransactionRequest, TxHash};
use tracing::info;

use crate::types::StakeEventData;

/// 计算 confirmEvent 函数的 4 字节选择器。
///
/// Solidity 函数签名：
/// `confirmEvent((bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64))`
///
/// 选择器 = keccak256(函数签名) 的前 4 字节。
fn confirm_event_selector() -> [u8; 4] {
    let sig = "confirmEvent((bytes32,bytes32,uint64,uint64,uint64,uint64,bytes32,bytes32,uint64))";
    let hash = ethers::utils::keccak256(sig.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// 计算 nonceConfirmations 函数的 4 字节选择器。
///
/// Solidity 函数签名：`nonceConfirmations(uint64)`
/// 返回值包含 isProcessed 等确认状态字段。
fn nonce_confirmations_selector() -> [u8; 4] {
    let sig = "nonceConfirmations(uint64)";
    let hash = ethers::utils::keccak256(sig.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// 将 StakeEventData ABI 编码为 confirmEvent 的 calldata。
///
/// 编码布局（共 4 + 9×32 = 292 字节）：
/// ```text
/// [4B 选择器]
/// [32B sourceContract]     ← bytes32
/// [32B targetContract]     ← bytes32
/// [32B sourceChainId]      ← uint64, 大端序右对齐
/// [32B targetChainId]      ← uint64
/// [32B blockHeight]        ← uint64
/// [32B amount]             ← uint64
/// [32B sender]             ← bytes32
/// [32B receiver]           ← bytes32
/// [32B nonce]              ← uint64
/// ```
fn encode_confirm_event(event: &StakeEventData) -> Vec<u8> {
    let mut calldata = Vec::with_capacity(4 + 9 * 32);
    calldata.extend_from_slice(&confirm_event_selector());

    // bytes32 字段直接写入
    calldata.extend_from_slice(&event.source_contract);
    calldata.extend_from_slice(&event.target_contract);

    // uint64 字段：放在 32 字节 word 的后 8 字节，前 24 字节为 0
    let mut word = [0u8; 32];
    word[24..32].copy_from_slice(&event.source_chain_id.to_be_bytes());
    calldata.extend_from_slice(&word);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.target_chain_id.to_be_bytes());
    calldata.extend_from_slice(&word);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.block_height.to_be_bytes());
    calldata.extend_from_slice(&word);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.amount.to_be_bytes());
    calldata.extend_from_slice(&word);

    calldata.extend_from_slice(&event.sender);
    calldata.extend_from_slice(&event.receiver);

    word = [0u8; 32];
    word[24..32].copy_from_slice(&event.nonce.to_be_bytes());
    calldata.extend_from_slice(&word);

    calldata
}

/// 检查某个 nonce 是否已在 EVM 链上被处理。
///
/// 调用合约的 `nonceConfirmations(uint64)` 视图函数（eth_call，不消耗 gas）。
/// 返回值是一个 struct，第一个字段 isProcessed 是 bool（第 32 字节的最后 1 位）。
pub async fn check_nonce_processed(
    provider: &Provider<Http>,
    contract: Address,
    nonce: u64,
) -> Result<bool> {
    // 编码 calldata：4B 选择器 + 32B nonce
    let mut calldata = Vec::with_capacity(4 + 32);
    calldata.extend_from_slice(&nonce_confirmations_selector());
    let mut word = [0u8; 32];
    word[24..32].copy_from_slice(&nonce.to_be_bytes());
    calldata.extend_from_slice(&word);

    // 构造 eth_call 请求（只读调用，不发送交易）
    let tx = TypedTransaction::Legacy(TransactionRequest::new().to(contract).data(calldata));

    let result = provider.call(&tx, None).await.context("调用 nonceConfirmations 失败")?;

    if result.len() < 32 {
        anyhow::bail!("nonceConfirmations 返回值太短: {} 字节", result.len());
    }

    // isProcessed 是返回 struct 的第一个字段（bool），在第一个 32 字节 word 的最后一字节
    let is_processed = result[31] != 0;
    Ok(is_processed)
}

/// 提交 confirmEvent 交易到 EVM 链。
///
/// 使用 SignerMiddleware 自动签名并发送交易。
/// `chain_id` 用于 EIP-155 签名保护（防止跨链重放）。
pub async fn submit_confirm_event(
    wallet: &LocalWallet,
    provider: &Provider<Http>,
    contract: Address,
    chain_id: u64,
    event: &StakeEventData,
) -> Result<TxHash> {
    // 设置钱包的 chain_id（EIP-155 签名保护）
    let wallet = wallet.clone().with_chain_id(chain_id);
    // 创建带签名能力的客户端
    let client = SignerMiddleware::new(provider.clone(), wallet);

    let calldata = encode_confirm_event(event);

    let tx = TransactionRequest::new()
        .to(contract)
        .data(calldata);

    // 发送交易（自动估算 gas、获取 nonce、签名、广播）
    let pending = client
        .send_transaction(tx, None)
        .await
        .context("发送 confirmEvent 交易失败")?;

    let tx_hash = pending.tx_hash();
    info!(
        nonce = event.nonce,
        tx_hash = ?tx_hash,
        "已提交 EVM confirmEvent 交易"
    );

    Ok(tx_hash)
}
