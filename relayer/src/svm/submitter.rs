//! SVM 确认指令提交模块
//!
//! 负责：
//! 1. 检查某个 nonce 是否已在 SVM 链上被确认（读取 CrossChainRequest PDA）
//! 2. 构建并发送 confirm_event 指令到桥合约
//!
//! PDA（Program Derived Address）说明：
//! - bridge_state：seeds=["bridge_state"]，全局状态
//! - peer_config：seeds=["peer_config", chain_id.to_le_bytes()]，对端链配置
//! - cross_chain_request：seeds=["cross_chain_request", source_chain_id, nonce]，跨链请求状态
//! - vault：seeds=["vault"]，合约的 USDC 金库

use anyhow::{Context, Result};
use borsh::BorshSerialize;
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;
use tracing::info;

use crate::types::StakeEventData;

/// SPL Associated Token Account 程序的地址（硬编码常量）
const SPL_ASSOCIATED_TOKEN_ACCOUNT_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

/// 计算 Anchor 指令的鉴别器。
/// Anchor 约定：SHA-256("global:{函数名}") 的前 8 字节。
fn confirm_event_discriminator() -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update("global:confirm_event");
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

/// 派生 BridgeState PDA 地址：seeds=["bridge_state"]
pub fn bridge_state_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"bridge_state"], program_id)
}

/// 派生 PeerConfig PDA 地址：seeds=["peer_config", chain_id(LE)]
pub fn peer_config_pda(program_id: &Pubkey, chain_id: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"peer_config", &chain_id.to_le_bytes()],
        program_id,
    )
}

/// 派生 CrossChainRequest PDA 地址：seeds=["cross_chain_request", source_chain_id(LE), nonce(LE)]
///
/// 每个 (source_chain_id, nonce) 对应一个唯一的 PDA，记录该跨链请求的确认状态。
pub fn cross_chain_request_pda(
    program_id: &Pubkey,
    source_chain_id: u64,
    nonce: u64,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            b"cross_chain_request",
            &source_chain_id.to_le_bytes(),
            &nonce.to_le_bytes(),
        ],
        program_id,
    )
}

/// 派生 Vault PDA 地址：seeds=["vault"]
///
/// Vault 是桥合约的 USDC 金库签名者（PDA 作为 authority）。
pub fn vault_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"vault"], program_id)
}

/// 检查某个 nonce 是否已在 SVM 链上被处理。
///
/// 原理：读取 CrossChainRequest PDA 账户数据，解析 is_processed 字段。
///
/// CrossChainRequest 账户的内存布局（Anchor）：
/// ```text
/// [8B disc] [8B nonce]
/// [4B relayer_count] [relayer_count × 32B confirmed_relayers]
/// [4B vote_count] [vote_count × 33B hash_votes (32B hash + 1B count)]
/// [1B frozen_threshold] [1B is_unlocked] [1B is_processed]
/// ```
///
/// 如果 PDA 账户不存在（value=None），说明该 nonce 尚未有任何 relayer 确认过。
pub fn check_nonce_processed(
    rpc: &RpcClient,
    program_id: &Pubkey,
    source_chain_id: u64,
    nonce: u64,
) -> Result<bool> {
    let (pda, _) = cross_chain_request_pda(program_id, source_chain_id, nonce);

    match rpc.get_account_with_commitment(&pda, CommitmentConfig::finalized())? {
        // 账户不存在 → 未处理
        solana_client::rpc_response::Response { value: None, .. } => Ok(false),
        // 账户存在 → 解析 is_processed 字段
        solana_client::rpc_response::Response {
            value: Some(account),
            ..
        } => {
            let data = &account.data;
            if data.len() < 8 + 8 + 4 {
                return Ok(false);
            }

            // 跳过鉴别器(8B) + nonce(8B)
            let mut offset = 8 + 8;

            // 跳过 confirmed_relayers: Vec<Pubkey>
            let relayer_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
            offset += 4 + relayer_count * 32;

            if offset + 4 > data.len() {
                return Ok(false);
            }

            // 跳过 hash_votes: Vec<HashVote>，每个 HashVote = 32B hash + 1B count = 33B
            let vote_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
            offset += 4 + vote_count * 33;

            if offset + 3 > data.len() {
                return Ok(false);
            }

            // 读取最后三个 bool 字段：frozen_threshold(1B) + is_unlocked(1B) + is_processed(1B)
            let is_processed = data[offset + 2] != 0;
            Ok(is_processed)
        }
    }
}

/// 构建并发送 confirm_event 指令到 SVM 桥合约。
///
/// 指令数据布局：
/// ```text
/// [8B 鉴别器] [8B nonce (LE)] [8B source_chain_id (LE)] [168B Borsh(StakeEventData)]
/// ```
///
/// 需要传入的账户列表（与 Anchor 合约的 ConfirmEvent context 一致）：
/// 1. bridge_state (writable) —— 全局状态
/// 2. peer_config (writable) —— 源链的配置
/// 3. cross_chain_request (writable) —— 跨链请求状态（会被创建或更新）
/// 4. relayer (signer, writable) —— relayer 的公钥
/// 5. vault (readonly) —— 金库 PDA
/// 6. usdc_mint (readonly) —— USDC 的 mint 地址
/// 7. vault_token_account (writable) —— 金库的 USDC ATA
/// 8. receiver_token_account (writable) —— 接收者的 USDC ATA
/// 9. token_program (readonly) —— SPL Token 或 Token-2022 程序
/// 10. system_program (readonly) —— 系统程序
pub fn submit_confirm_event(
    rpc: &RpcClient,
    program_id: &Pubkey,
    relayer_keypair: &Keypair,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    event: &StakeEventData,
) -> Result<Signature> {
    // 派生所有需要的 PDA 地址
    let (bridge_state, _) = bridge_state_pda(program_id);
    let (peer_config, _) = peer_config_pda(program_id, event.source_chain_id);
    let (cross_chain_request, _) =
        cross_chain_request_pda(program_id, event.source_chain_id, event.nonce);
    let (vault, _) = vault_pda(program_id);

    // 计算金库和接收者的 Associated Token Account (ATA) 地址
    let vault_token_account = spl_associated_token_account_address(&vault, usdc_mint, token_program_id);
    let receiver_pubkey = Pubkey::new_from_array(event.receiver);
    let receiver_token_account = spl_associated_token_account_address(&receiver_pubkey, usdc_mint, token_program_id);

    // 构建指令数据：鉴别器 + nonce + source_chain_id + Borsh(event)
    let mut ix_data = Vec::with_capacity(8 + 8 + 8 + StakeEventData::BORSH_LEN);
    ix_data.extend_from_slice(&confirm_event_discriminator());
    ix_data.extend_from_slice(&event.nonce.to_le_bytes());
    ix_data.extend_from_slice(&event.source_chain_id.to_le_bytes());
    event.serialize(&mut ix_data)?;

    // 构建账户列表（顺序必须与合约 context 定义一致）
    let accounts = vec![
        AccountMeta::new(bridge_state, false),                    // 全局状态（可写）
        AccountMeta::new(peer_config, false),                     // 对端链配置（可写）
        AccountMeta::new(cross_chain_request, false),             // 跨链请求状态（可写）
        AccountMeta::new(relayer_keypair.pubkey(), true),         // relayer（签名者+可写）
        AccountMeta::new_readonly(vault, false),                  // 金库 PDA（只读）
        AccountMeta::new_readonly(*usdc_mint, false),             // USDC mint（只读）
        AccountMeta::new(vault_token_account, false),             // 金库 ATA（可写）
        AccountMeta::new(receiver_token_account, false),          // 接收者 ATA（可写）
        AccountMeta::new_readonly(*token_program_id, false),      // Token 程序（只读）
        AccountMeta::new_readonly(solana_sdk::system_program::id(), false), // 系统程序（只读）
    ];

    let ix = Instruction::new_with_bytes(*program_id, &ix_data, accounts);

    // 获取最近的 blockhash 并构建签名交易
    let recent_blockhash = rpc.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&relayer_keypair.pubkey()),
        &[relayer_keypair],
        recent_blockhash,
    );

    // 发送并等待确认
    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .context("发送 confirm_event 交易失败")?;

    info!(
        nonce = event.nonce,
        tx = %sig,
        "已提交 SVM confirm_event 交易"
    );

    Ok(sig)
}

/// 计算 SPL Associated Token Account (ATA) 的地址。
///
/// ATA 是一个确定性派生的地址，由以下 seeds 计算：
/// - wallet：所有者地址
/// - token_program_id：Token 程序 ID（SPL Token 或 Token-2022）
/// - mint：代币 mint 地址
///
/// ATA 程序 ID 是固定的：ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL
fn spl_associated_token_account_address(wallet: &Pubkey, mint: &Pubkey, token_program_id: &Pubkey) -> Pubkey {
    let ata_program_id: Pubkey = SPL_ASSOCIATED_TOKEN_ACCOUNT_ID.parse().unwrap();
    Pubkey::find_program_address(
        &[
            wallet.as_ref(),
            token_program_id.as_ref(),
            mint.as_ref(),
        ],
        &ata_program_id,
    )
    .0
}
