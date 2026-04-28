//! SVM 确认指令提交模块（pipelined submit + async confirmation 模型）。
//!
//! 三件事：
//! 1. [`check_nonce_status`] —— 读 CrossChainRequest PDA 判断 nonce 状态（含自投票检测）
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

use crate::types::BridgeEventData;

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

/// SPL Associated Token Account 程序的 `CreateIdempotent` 指令鉴别器。
///
/// 单字节即指令编号：
/// - 0 = Create（已存在则报错）
/// - 1 = CreateIdempotent（已存在则 no-op，本场景所需）
/// - 2 = RecoverNested
const ATA_CREATE_IDEMPOTENT_DISCRIMINATOR: u8 = 1;

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

/// SVM 上某个 (source_chain_id, nonce) 对应的链上状态，供 submitter 决策。
///
/// 三态与 EVM 的 `NonceStatus` 对齐。
/// "PDA 不存在"合并进 `PendingOurVote`（语义相同：需要广播）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonceStatus {
    /// is_processed == true → 事件已完全处理（投票达到阈值并已 unlock）
    FullyProcessed,
    /// is_processed == false，但指定 relayer 已在 confirmed_relayers 中 →
    /// 我们已投过票，不应重复广播，等待其他 relayer 投票达到阈值即可
    AlreadyConfirmedByUs,
    /// PDA 不存在或 is_processed == false 且本 relayer 未投票 → 需要广播 confirm_event
    PendingOurVote,
}

/// 检查某个 nonce 在 SVM 上的状态，同时判断指定 relayer 是否已投票。
///
/// 一次 RPC 调用读取 CrossChainRequest PDA，解析 `is_processed` 和
/// `confirmed_relayers` 两个字段，返回 [`NonceStatus`]。
///
/// `commitment` 控制查询的一致性级别：
/// - `confirmed`：用于 Branch A step1 快速感知（避免 finalized 窗口漏检）
/// - `finalized`：用于最终确认/删文件（防回滚）
///
/// CrossChainRequest 账户的内存布局（Anchor）：
/// ```text
/// [8B disc] [8B nonce]
/// [4B relayer_count] [relayer_count × 32B confirmed_relayers]
/// [4B vote_count] [vote_count × 33B hash_votes (32B hash + 1B count)]
/// [1B frozen_threshold] [1B is_unlocked] [1B is_processed]
/// ```
pub async fn check_nonce_status(
    rpc: &RpcClient,
    program_id: &Pubkey,
    source_chain_id: u64,
    nonce: u64,
    relayer_pubkey: &Pubkey,
    commitment: CommitmentConfig,
) -> Result<NonceStatus> {
    let (pda, _) = cross_chain_request_pda(program_id, source_chain_id, nonce);

    match rpc
        .get_account_with_commitment(&pda, commitment)
        .await?
    {
        solana_client::rpc_response::Response { value: None, .. } => Ok(NonceStatus::PendingOurVote),
        solana_client::rpc_response::Response {
            value: Some(account),
            ..
        } => parse_nonce_status(&account.data, relayer_pubkey),
    }
}

/// 从 CrossChainRequest PDA 的原始字节中解析 NonceStatus。
///
/// 抽出来方便单测验证解析逻辑（不需要 mock RPC）。
fn parse_nonce_status(data: &[u8], relayer_pubkey: &Pubkey) -> Result<NonceStatus> {
    if data.len() < 8 + 8 + 4 {
        return Ok(NonceStatus::PendingOurVote);
    }

    // 跳过鉴别器(8B) + nonce(8B)
    let mut offset = 8 + 8;

    // 解析 confirmed_relayers: Vec<Pubkey>
    let relayer_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    let mut already_confirmed = false;
    for i in 0..relayer_count {
        let start = offset + i * 32;
        if start + 32 > data.len() {
            break;
        }
        if &data[start..start + 32] == relayer_pubkey.as_ref() {
            already_confirmed = true;
            break;
        }
    }
    offset += relayer_count * 32;

    // 跳过 hash_votes: Vec<HashVote>，每个 HashVote = 32B hash + 1B count = 33B
    if offset + 4 > data.len() {
        return Ok(if already_confirmed {
            NonceStatus::AlreadyConfirmedByUs
        } else {
            NonceStatus::PendingOurVote
        });
    }
    let vote_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4 + vote_count * 33;

    if offset + 3 > data.len() {
        return Ok(if already_confirmed {
            NonceStatus::AlreadyConfirmedByUs
        } else {
            NonceStatus::PendingOurVote
        });
    }

    // 读取最后三个 bool 字段：frozen_threshold(1B) + is_unlocked(1B) + is_processed(1B)
    let is_processed = data[offset + 2] != 0;

    if is_processed {
        Ok(NonceStatus::FullyProcessed)
    } else if already_confirmed {
        Ok(NonceStatus::AlreadyConfirmedByUs)
    } else {
        Ok(NonceStatus::PendingOurVote)
    }
}

/// 构建 `confirm_event` 指令的 calldata + AccountMeta 列表。
///
/// 与 `broadcast_confirm_event` 共用，但抽出来方便单测验证编码长度/字段位置。
///
/// 指令数据布局：
/// ```text
/// [8B 鉴别器] [8B nonce (LE)] [8B source_chain_id (LE)] [176B Borsh(BridgeEventData)]
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
    event: &BridgeEventData,
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

    let mut ix_data = Vec::with_capacity(8 + 8 + 8 + BridgeEventData::BORSH_LEN);
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
///    直接返回 Err，上层下一轮 `check_nonce_status` 会自动收尾删文件
/// 3. 上层把 (signature, sent_at_unix) 写到事件文件的 submission 字段
///
/// 后续等待 finalized / 检测 dropped 由 [`check_tx_maturity`] 在后续轮次完成。
///
/// 指令组成（按顺序两条 ix）：
/// 1. ATA `CreateIdempotent`：保证接收方 USDC ATA 在 confirm_event 执行时一定存在。
///    已存在则 no-op，不存在则由 relayer 付 ~0.00204 SOL rent 创建。
///    避免"目标地址首次接收 USDC 时 ATA 缺失导致 confirm_event 反序列化失败"这种卡死状态。
/// 2. `confirm_event`：投票确认事件，达到阈值时自动 unlock 到接收方 ATA。
///
/// 这两条 ix 在同一笔 tx 内顺序执行，create 后的 ATA 状态对 confirm_event 立即可见。
pub async fn broadcast_confirm_event(
    rpc: &RpcClient,
    program_id: &Pubkey,
    relayer_keypair: &Keypair,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    event: &BridgeEventData,
) -> Result<Signature> {
    let receiver_pubkey = Pubkey::new_from_array(event.receiver);

    let create_ata_ix = build_create_ata_idempotent_instruction(
        &relayer_keypair.pubkey(),
        &receiver_pubkey,
        usdc_mint,
        token_program_id,
    );
    let confirm_ix = build_confirm_event_instruction(
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
        &[create_ata_ix, confirm_ix],
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
///   要么链上确实没成功（重广播本就该做），要么已成功（下一轮 `check_nonce_status`
///   返回 FullyProcessed / AlreadyConfirmedByUs 触发删文件，重广播的那笔会被 preflight 直接 revert，不上链不收 gas）
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

/// 构建一条 SPL ATA 的 `CreateIdempotent` 指令，由调用方付 rent 创建 ATA。
///
/// 与普通 `Create` 的区别：当目标 ATA 已存在时不会 revert，而是 no-op，
/// 因此可以无脑塞在 `confirm_event` 之前——不需要先 `getAccountInfo` 探测。
///
/// 设计动机：当 EVM→SVM 跨链的接收方在 1024 链上还没有 USDC ATA 时，
/// `confirm_event` 中的 `InterfaceAccount<TokenAccount>` 反序列化会失败，
/// 导致 relayer preflight 永久卡住。让 relayer 在同一笔 tx 里先建好 ATA，
/// 即可完全消除这种"目标地址 ATA 缺失导致跨链卡死"的失败模式。
///
/// 多 relayer 并发投票时，只有第一个 land 的 relayer 真正承担 ~0.00204 SOL rent，
/// 后续 relayer 的同名指令命中 idempotent 分支只花 ~3000 CU。
///
/// 账户列表（ATA 程序的标准顺序）：
/// 1. funding (signer, writable) —— 付 rent 的账户，本场景为 relayer
/// 2. associated_token_account (writable) —— 派生出的 ATA 地址
/// 3. wallet (readonly) —— ATA 的 owner，本场景为 event_data.receiver
/// 4. mint (readonly) —— USDC mint
/// 5. system_program (readonly)
/// 6. token_program (readonly) —— SPL Token 或 Token-2022
fn build_create_ata_idempotent_instruction(
    funding: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program_id: &Pubkey,
) -> Instruction {
    let ata_program_id: Pubkey = SPL_ASSOCIATED_TOKEN_ACCOUNT_ID.parse().unwrap();
    let ata = spl_associated_token_account_address(wallet, mint, token_program_id);

    let accounts = vec![
        AccountMeta::new(*funding, true),
        AccountMeta::new(ata, false),
        AccountMeta::new_readonly(*wallet, false),
        AccountMeta::new_readonly(*mint, false),
        AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
        AccountMeta::new_readonly(*token_program_id, false),
    ];

    Instruction::new_with_bytes(
        ata_program_id,
        &[ATA_CREATE_IDEMPOTENT_DISCRIMINATOR],
        accounts,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::pubkey::Pubkey;

    fn dummy_program() -> Pubkey {
        Pubkey::new_from_array([1u8; 32])
    }

    /// 构造一个模拟的 CrossChainRequest 账户数据，用于测试 parse_nonce_status。
    ///
    /// 布局: [8B disc][8B nonce][4B relayer_count][relayers...][4B vote_count][votes...][3B bools]
    fn build_mock_ccr_data(
        confirmed_relayers: &[Pubkey],
        vote_count: u32,
        frozen_threshold: u8,
        is_unlocked: bool,
        is_processed: bool,
    ) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; 8]); // discriminator
        data.extend_from_slice(&42u64.to_le_bytes()); // nonce
        data.extend_from_slice(&(confirmed_relayers.len() as u32).to_le_bytes());
        for pk in confirmed_relayers {
            data.extend_from_slice(pk.as_ref());
        }
        data.extend_from_slice(&vote_count.to_le_bytes());
        for _ in 0..vote_count {
            data.extend_from_slice(&[0u8; 33]); // dummy HashVote
        }
        data.push(frozen_threshold);
        data.push(if is_unlocked { 1 } else { 0 });
        data.push(if is_processed { 1 } else { 0 });
        data
    }

    /// parse_nonce_status: PDA 不存在 / 数据太短 → PendingOurVote
    #[test]
    fn parse_nonce_status_returns_pending_for_short_data() {
        let pk = Pubkey::new_from_array([1u8; 32]);
        assert_eq!(parse_nonce_status(&[], &pk).unwrap(), NonceStatus::PendingOurVote);
        assert_eq!(parse_nonce_status(&[0u8; 19], &pk).unwrap(), NonceStatus::PendingOurVote);
    }

    /// parse_nonce_status: is_processed=true → FullyProcessed（不管 relayer 是否在列表中）
    #[test]
    fn parse_nonce_status_fully_processed() {
        let relayer = Pubkey::new_from_array([5u8; 32]);
        let data = build_mock_ccr_data(&[relayer], 1, 1, true, true);
        assert_eq!(parse_nonce_status(&data, &relayer).unwrap(), NonceStatus::FullyProcessed);

        let other = Pubkey::new_from_array([6u8; 32]);
        assert_eq!(parse_nonce_status(&data, &other).unwrap(), NonceStatus::FullyProcessed);
    }

    /// parse_nonce_status: is_processed=false + relayer 在 confirmed_relayers → AlreadyConfirmedByUs
    #[test]
    fn parse_nonce_status_already_confirmed() {
        let relayer = Pubkey::new_from_array([5u8; 32]);
        let other = Pubkey::new_from_array([6u8; 32]);
        let data = build_mock_ccr_data(&[other, relayer], 2, 2, false, false);
        assert_eq!(parse_nonce_status(&data, &relayer).unwrap(), NonceStatus::AlreadyConfirmedByUs);
    }

    /// parse_nonce_status: is_processed=false + relayer 不在 confirmed_relayers → PendingOurVote
    #[test]
    fn parse_nonce_status_pending_our_vote() {
        let relayer = Pubkey::new_from_array([5u8; 32]);
        let other = Pubkey::new_from_array([6u8; 32]);
        let data = build_mock_ccr_data(&[other], 1, 2, false, false);
        assert_eq!(parse_nonce_status(&data, &relayer).unwrap(), NonceStatus::PendingOurVote);
    }

    /// parse_nonce_status: 空 confirmed_relayers → PendingOurVote
    #[test]
    fn parse_nonce_status_empty_relayers() {
        let relayer = Pubkey::new_from_array([5u8; 32]);
        let data = build_mock_ccr_data(&[], 0, 0, false, false);
        assert_eq!(parse_nonce_status(&data, &relayer).unwrap(), NonceStatus::PendingOurVote);
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
    /// `[8B disc][8B nonce LE][8B source_chain_id LE][Borsh(BridgeEventData)]`
    /// 任何顺序错位都会让链上反序列化报错 / 走错分支。
    #[test]
    fn confirm_event_instruction_calldata_layout_is_correct() {
        let event = BridgeEventData {
            source_contract: [0xaa; 32],
            target_contract: [0xbb; 32],
            source_chain_id: 91024,
            target_chain_id: 1,
            block_height: 7,
            raw_amount: 1_000,
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
        assert_eq!(ix.data.len(), 24 + BridgeEventData::BORSH_LEN);
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

    /// `CreateIdempotent` 指令的字段布局必须与 SPL ATA 程序的 ABI 一致：
    /// - program_id == ATA program
    /// - data == 单字节 `1`（CreateIdempotent 编号）
    /// - 账户 6 个，顺序：funding(签名,可写) / ata(可写) / wallet / mint / system / token_program
    ///
    /// 任一字段错位都会让 ATA 程序直接报 InvalidInstruction 或 IncorrectAccount。
    #[test]
    fn create_ata_idempotent_instruction_layout_is_correct() {
        let funding = Pubkey::new_from_array([7u8; 32]);
        let wallet = Pubkey::new_from_array([8u8; 32]);
        let mint = Pubkey::new_from_array([9u8; 32]);
        let token_program: Pubkey =
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".parse().unwrap();

        let ix = build_create_ata_idempotent_instruction(&funding, &wallet, &mint, &token_program);

        let expected_program_id: Pubkey = SPL_ASSOCIATED_TOKEN_ACCOUNT_ID.parse().unwrap();
        assert_eq!(ix.program_id, expected_program_id);

        // 单字节 discriminator
        assert_eq!(ix.data, vec![ATA_CREATE_IDEMPOTENT_DISCRIMINATOR]);
        assert_eq!(ix.data, vec![1u8]);

        assert_eq!(ix.accounts.len(), 6);

        // funding 必须是签名者且可写
        assert_eq!(ix.accounts[0].pubkey, funding);
        assert!(ix.accounts[0].is_signer);
        assert!(ix.accounts[0].is_writable);

        // ata 必须可写但不签名，且地址等于派生结果
        let expected_ata = spl_associated_token_account_address(&wallet, &mint, &token_program);
        assert_eq!(ix.accounts[1].pubkey, expected_ata);
        assert!(!ix.accounts[1].is_signer);
        assert!(ix.accounts[1].is_writable);

        // wallet / mint / system / token_program 都是 readonly 非签名
        for i in 2..6 {
            assert!(!ix.accounts[i].is_signer);
            assert!(!ix.accounts[i].is_writable);
        }
        assert_eq!(ix.accounts[2].pubkey, wallet);
        assert_eq!(ix.accounts[3].pubkey, mint);
        assert_eq!(ix.accounts[4].pubkey, solana_sdk::system_program::id());
        assert_eq!(ix.accounts[5].pubkey, token_program);
    }

    /// `CreateIdempotent` 派生的 ATA 必须与 confirm_event 指令里使用的接收方 ATA 严格一致，
    /// 否则同一笔 tx 内"先建后用"的协作会把 USDC 转到一个空账户、留下被 GC 的孤儿 ATA。
    #[test]
    fn create_ata_target_matches_confirm_event_receiver_ata() {
        let event = BridgeEventData {
            source_contract: [0xaa; 32],
            target_contract: [0xbb; 32],
            source_chain_id: 1,
            target_chain_id: 91024,
            block_height: 1,
            raw_amount: 1_000_000,
            amount: 1_000_000,
            sender: [0xcc; 32],
            receiver: [0xdd; 32],
            nonce: 99,
        };
        let receiver = Pubkey::new_from_array(event.receiver);
        let mint = Pubkey::new_from_array([0xee; 32]);
        let token_program: Pubkey =
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".parse().unwrap();
        let funding = Pubkey::new_from_array([0xff; 32]);

        let create_ix =
            build_create_ata_idempotent_instruction(&funding, &receiver, &mint, &token_program);
        let confirm_ix = build_confirm_event_instruction(
            &dummy_program(),
            &funding,
            &mint,
            &token_program,
            &event,
        )
        .expect("build confirm ok");

        // ATA 程序里 accounts[1] 是被创建的 ATA；confirm_event 里 accounts[7] 是 receiver_token_account
        // 二者必须严格相等
        assert_eq!(
            create_ix.accounts[1].pubkey, confirm_ix.accounts[7].pubkey,
            "create 的 ATA 必须和 confirm_event 引用的 receiver_token_account 是同一个"
        );
    }
}
