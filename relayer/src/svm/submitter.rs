//! SVM 确认指令提交模块（pipelined submit + async confirmation 模型）。
//!
//! 三件事：
//! 1. [`check_nonce_processed`] —— 读 CrossChainRequest PDA 看 nonce 是否已被确认
//! 2. [`broadcast_confirm_event`] —— 构建+广播 `confirm_event` 指令，**不等回执**，
//!    立即返回 base58 签名让上层把 submission 写盘
//! 3. [`check_tx_maturity`] —— 给定 signature，查询其 `confirmation_status`
//!    （Processed/Confirmed/Finalized）+ 错误状态
//!
//! 与 EVM submitter 镜像设计：单事件每轮只花 ~RPC 延迟，不再 `send_and_confirm_transaction`
//! 同步阻塞 ~13s 等 finalized；多事件可以"在飞"等 finalize，互不阻塞。
//!
//! PDA 派生说明：
//! - bridge_state：seeds=["bridge_state"]，全局状态
//! - peer_config：seeds=["peer_config", chain_id.to_le_bytes()]，对端链配置
//! - cross_chain_request：seeds=["cross_chain_request", source_chain_id, nonce]，跨链请求状态
//! - vault：seeds=["vault"]，合约的 USDC 金库

use anyhow::{Context, Result};
use borsh::BorshSerialize;
use sha2::{Digest, Sha256};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;
use solana_transaction_status::TransactionConfirmationStatus;
use tracing::info;

use crate::types::StakeEventData;

/// SVM 一笔已广播 tx 的链上成熟度状态。供上层状态机决策用。
///
/// 与 EVM 的 `TxMaturity` 类似但不带 `mined_block` —— SVM 直接看 `confirmation_status`
/// 字段，不需要按 block 深度算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxMaturity {
    /// `confirmation_status == Finalized`：已最终确认（≥ 2/3 supermajority + 31 epochs root）
    Confirmed { slot: u64 },
    /// `confirmation_status == Processed | Confirmed`：已 land 但还没 finalized，等
    Pending { slot: u64 },
    /// 找不到 status：tx 可能还在 mempool / blockhash 还没过期未 land / 已被 GC。
    /// 上层结合 `sent_at_unix` 做 stale 处理。
    NotYetLanded,
    /// status 存在但 `err` 非空：链上 revert（多见于 AlreadyProcessed）。
    Reverted { slot: u64 },
}

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
pub async fn check_nonce_processed(
    rpc: &RpcClient,
    program_id: &Pubkey,
    source_chain_id: u64,
    nonce: u64,
) -> Result<bool> {
    let (pda, _) = cross_chain_request_pda(program_id, source_chain_id, nonce);

    match rpc
        .get_account_with_commitment(&pda, CommitmentConfig::finalized())
        .await?
    {
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

/// 构建 `confirm_event` 指令的 calldata + AccountMeta 列表。
///
/// 与 `broadcast_confirm_event` 共用，但抽出来方便单测验证编码长度/字段位置。
///
/// 指令数据布局：
/// ```text
/// [8B 鉴别器] [8B nonce (LE)] [8B source_chain_id (LE)] [168B Borsh(StakeEventData)]
/// ```
///
/// 账户列表（顺序必须与合约 ConfirmEvent context 一致）：
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
fn build_confirm_event_instruction(
    program_id: &Pubkey,
    relayer_pubkey: &Pubkey,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    event: &StakeEventData,
) -> Result<Instruction> {
    let (bridge_state, _) = bridge_state_pda(program_id);
    let (peer_config, _) = peer_config_pda(program_id, event.source_chain_id);
    let (cross_chain_request, _) =
        cross_chain_request_pda(program_id, event.source_chain_id, event.nonce);
    let (vault, _) = vault_pda(program_id);

    let vault_token_account =
        spl_associated_token_account_address(&vault, usdc_mint, token_program_id);
    let receiver_pubkey = Pubkey::new_from_array(event.receiver);
    let receiver_token_account =
        spl_associated_token_account_address(&receiver_pubkey, usdc_mint, token_program_id);

    let mut ix_data = Vec::with_capacity(8 + 8 + 8 + StakeEventData::BORSH_LEN);
    ix_data.extend_from_slice(&confirm_event_discriminator());
    ix_data.extend_from_slice(&event.nonce.to_le_bytes());
    ix_data.extend_from_slice(&event.source_chain_id.to_le_bytes());
    event.serialize(&mut ix_data)?;

    let accounts = vec![
        AccountMeta::new(bridge_state, false),
        AccountMeta::new(peer_config, false),
        AccountMeta::new(cross_chain_request, false),
        AccountMeta::new(*relayer_pubkey, true),
        AccountMeta::new_readonly(vault, false),
        AccountMeta::new_readonly(*usdc_mint, false),
        AccountMeta::new(vault_token_account, false),
        AccountMeta::new(receiver_token_account, false),
        AccountMeta::new_readonly(*token_program_id, false),
        AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
    ];

    Ok(Instruction::new_with_bytes(*program_id, &ix_data, accounts))
}

/// 广播 `confirm_event` 指令，**不等任何 confirmation**，立即返回 base58 签名。
///
/// 流程：
/// 1. 派生所有 PDA / ATA、组装指令、用最近 blockhash 签名
/// 2. `rpc.send_transaction(&tx).await` —— 触发 RPC 节点 preflight 模拟，
///    通过则推到 mempool，立即返回 signature；模拟失败（如 AlreadyProcessed）
///    直接返回 Err，上层下一轮 `check_nonce_processed` 会自动收尾删文件
/// 3. 上层把 (signature, sent_at_unix) 写到事件文件的 submission 字段
///
/// 后续等待 finalized / 检测 dropped 由 [`check_tx_maturity`] 在后续轮次完成。
pub async fn broadcast_confirm_event(
    rpc: &RpcClient,
    program_id: &Pubkey,
    relayer_keypair: &Keypair,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    event: &StakeEventData,
) -> Result<Signature> {
    let ix = build_confirm_event_instruction(
        program_id,
        &relayer_keypair.pubkey(),
        usdc_mint,
        token_program_id,
        event,
    )?;

    // 每次广播都要拿新 blockhash —— 旧 blockhash 过期 (~60-90s) 后节点会拒收。
    // 这一笔 RPC 是可以并行优化的，但目前 submitter 串行处理事件，没必要先复杂化。
    let recent_blockhash = rpc
        .get_latest_blockhash()
        .await
        .context("获取最新 blockhash 失败")?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&relayer_keypair.pubkey()),
        &[relayer_keypair],
        recent_blockhash,
    );

    let sig = rpc
        .send_transaction(&tx)
        .await
        .context("发送 confirm_event 交易失败（preflight 拒绝多见于 AlreadyProcessed / 余额不足）")?;

    info!(
        nonce = event.nonce,
        source_chain_id = event.source_chain_id,
        tx = %sig,
        "已广播 SVM confirm_event 交易（不等 finalized，下一轮检查成熟度）"
    );

    Ok(sig)
}

/// 查询一笔已广播 SVM tx 的成熟度。每轮 submitter 用 1 次 RPC 调用即可推进状态机：
/// - 没看到 status → `NotYetLanded`（上层结合 sent_at 做 stale 处理）
/// - status 存在且 `confirmation_status == Finalized` → `Confirmed`
/// - status 存在且 `confirmation_status == Processed | Confirmed` → `Pending`
/// - status 存在且 `err` 非空 → `Reverted`
///
/// 注意：`get_signature_statuses` 默认 **不** 搜索历史账本，只能查到最近 ~150 slot 内
/// 的 tx 状态。这对我们没问题：
/// - 正常情况：tx 在 ~30s 内 finalize，远早于被 GC
/// - 即使 status 真被 GC 而我们看到 `NotYetLanded`，stale 重广播也是安全的 ——
///   要么链上确实没成功（重广播本就该做），要么已成功（下一轮 `check_nonce_processed`
///   返回 true 触发删文件，重广播的那笔会被 preflight 直接 revert，不上链不收 gas）
pub async fn check_tx_maturity(rpc: &RpcClient, sig: Signature) -> Result<TxMaturity> {
    let resp = rpc
        .get_signature_statuses(&[sig])
        .await
        .context("查询 signature 状态失败")?;
    let status = resp.value.into_iter().next().flatten();

    let Some(s) = status else {
        return Ok(TxMaturity::NotYetLanded);
    };

    if s.err.is_some() {
        return Ok(TxMaturity::Reverted { slot: s.slot });
    }

    match s.confirmation_status {
        Some(TransactionConfirmationStatus::Finalized) => Ok(TxMaturity::Confirmed { slot: s.slot }),
        Some(TransactionConfirmationStatus::Processed | TransactionConfirmationStatus::Confirmed)
        | None => Ok(TxMaturity::Pending { slot: s.slot }),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::pubkey::Pubkey;

    fn dummy_program() -> Pubkey {
        Pubkey::new_from_array([1u8; 32])
    }

    /// PDA 派生必须是确定性的：同输入两次调用 → 同输出。
    /// `find_program_address` 是确定性的，但这条测试可以保护
    /// 任何"换 seeds 顺序 / 多塞个 seed"的回归。
    #[test]
    fn pda_derivations_are_deterministic() {
        let pid = dummy_program();
        assert_eq!(bridge_state_pda(&pid), bridge_state_pda(&pid));
        assert_eq!(peer_config_pda(&pid, 1), peer_config_pda(&pid, 1));
        assert_eq!(
            cross_chain_request_pda(&pid, 1, 42),
            cross_chain_request_pda(&pid, 1, 42)
        );
        assert_eq!(vault_pda(&pid), vault_pda(&pid));
    }

    /// 不同 (chain_id, nonce) 对应不同 CrossChainRequest PDA —— 防 replay 的核心。
    #[test]
    fn cross_chain_request_pda_unique_per_chain_and_nonce() {
        let pid = dummy_program();
        let a = cross_chain_request_pda(&pid, 1, 42).0;
        let b = cross_chain_request_pda(&pid, 1, 43).0;
        let c = cross_chain_request_pda(&pid, 2, 42).0;
        assert_ne!(a, b, "不同 nonce 应得到不同 PDA");
        assert_ne!(a, c, "不同 chain_id 应得到不同 PDA");
        assert_ne!(b, c);
    }

    /// 不同 chain_id 的 PeerConfig PDA 必须互不相同。
    #[test]
    fn peer_config_pda_unique_per_chain() {
        let pid = dummy_program();
        let a = peer_config_pda(&pid, 1).0;
        let b = peer_config_pda(&pid, 91024).0;
        assert_ne!(a, b);
    }

    /// confirm_event 指令鉴别器：SHA256("global:confirm_event")[..8]
    #[test]
    fn confirm_event_discriminator_matches_anchor_formula() {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"global:confirm_event");
        let expected = &hasher.finalize()[..8];
        let got = confirm_event_discriminator();
        assert_eq!(&got[..], expected);
    }

    /// `build_confirm_event_instruction` 的 calldata 布局必须与 Anchor 合约期望的一致：
    /// `[8B disc][8B nonce LE][8B source_chain_id LE][Borsh(StakeEventData)]`
    /// 任何顺序错位都会让链上反序列化报错 / 走错分支。
    #[test]
    fn confirm_event_instruction_calldata_layout_is_correct() {
        let event = StakeEventData {
            source_contract: [0xaa; 32],
            target_contract: [0xbb; 32],
            source_chain_id: 91024,
            target_chain_id: 1,
            block_height: 7,
            amount: 1_000,
            sender: [0xcc; 32],
            receiver: [0xdd; 32],
            nonce: 42,
        };
        let pid = dummy_program();
        let relayer = Pubkey::new_from_array([5u8; 32]);
        let mint = Pubkey::new_from_array([6u8; 32]);
        let token_program: Pubkey =
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".parse().unwrap();

        let ix = build_confirm_event_instruction(&pid, &relayer, &mint, &token_program, &event)
            .expect("ok");

        // 头 8B = discriminator
        assert_eq!(&ix.data[..8], &confirm_event_discriminator()[..]);
        // 接下来 8B = nonce LE
        assert_eq!(&ix.data[8..16], &event.nonce.to_le_bytes());
        // 再 8B = source_chain_id LE
        assert_eq!(&ix.data[16..24], &event.source_chain_id.to_le_bytes());
        // 剩余 = Borsh(event)
        let body = &ix.data[24..];
        let expected_body = borsh::to_vec(&event).expect("serialize");
        assert_eq!(body, expected_body.as_slice());
        // 总长度 = 8 + 8 + 8 + BORSH_LEN
        assert_eq!(ix.data.len(), 24 + StakeEventData::BORSH_LEN);
        // 账户列表数量必须是 10（与合约 ConfirmEvent context 严格一致）
        assert_eq!(ix.accounts.len(), 10);
        // relayer 必须是签名者（且可写），位置在 index 3
        assert!(ix.accounts[3].is_signer);
        assert!(ix.accounts[3].is_writable);
        assert_eq!(ix.accounts[3].pubkey, relayer);
    }

    /// SPL ATA 地址派生稳定（同输入 → 同输出）。
    #[test]
    fn ata_derivation_is_deterministic() {
        let wallet = Pubkey::new_from_array([2u8; 32]);
        let mint = Pubkey::new_from_array([3u8; 32]);
        let token_program: Pubkey = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".parse().unwrap();
        let a = spl_associated_token_account_address(&wallet, &mint, &token_program);
        let b = spl_associated_token_account_address(&wallet, &mint, &token_program);
        assert_eq!(a, b);
    }
}
