//! SVM 事件轮询模块（三段式流水线的前两段逻辑）
//!
//! - `enumerate_new_signatures`：分页获取桥合约新签名（跳过链上失败 tx），
//!   供 sig enumerator task 使用。
//! - `fetch_and_extract_events`：按单个签名拉取交易日志并解析 Anchor 格式
//!   的 Staked，供 event extractor task 使用。
//!
//! 核心特点：
//! - 分页获取签名（batch_size 控制每页大小），**每轮一直翻到 checkpoint 为止**，
//!   不做总量截断 —— 保证返回列表与 checkpoint 连续、绝不漏签名
//! - 使用 finalized 确认级别，只处理已最终确认的交易
//! - 自动识别 Anchor 的 "Program data:" 日志前缀和事件鉴别器，且只认桥合约
//!   **自己**打出的行（按调用栈归属，#1133），并用链上 StakeRecord 佐证

use anyhow::{Context, Result};
use base64::Engine;
use sha2::{Digest, Sha256};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_transaction_status::UiTransactionEncoding;
use tracing::{debug, error, warn};

use crate::svm::log_attribution::attribute_program_data;
use crate::svm::stake_proof::verify_staked_against_chain;
use crate::types::{BridgeEventData, SvmProgramKind};

/// 计算 Anchor 事件的鉴别器。
/// Anchor 约定：SHA-256("event:{事件名}") 的前 8 字节。
fn staked_discriminator() -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update("event:Staked");
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

/// 从 Anchor 程序日志数据中解析 Staked。
///
/// 数据布局：
/// ```text
/// [8B 鉴别器] [Borsh 序列化的 BridgeEventData (176B)]
/// ```
///
/// BridgeEventData 的 Borsh 布局（小端序）：
/// - source_contract: [u8; 32]
/// - target_contract: [u8; 32]
/// - source_chain_id: u64 (8B LE)
/// - target_chain_id: u64
/// - block_height: u64
/// - raw_amount: u64
/// - amount: u64
/// - sender: [u8; 32]
/// - receiver: [u8; 32]
/// - nonce: u64
fn parse_staked_from_data(data: &[u8]) -> Result<BridgeEventData> {
    let disc = staked_discriminator();
    if data.len() < 8 + BridgeEventData::BORSH_LEN {
        anyhow::bail!("Staked 数据太短: {} 字节", data.len());
    }
    // 检查鉴别器是否匹配
    if data[..8] != disc {
        anyhow::bail!("不是 Staked（鉴别器不匹配）");
    }

    // 跳过 8 字节鉴别器，开始逐字段解析
    let body = &data[8..];
    let mut offset = 0;

    let mut source_contract = [0u8; 32];
    source_contract.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    let mut target_contract = [0u8; 32];
    target_contract.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    // 注意：Borsh/SVM 使用小端序（LE），与 EVM 的大端序（BE）不同
    let source_chain_id = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;
    let target_chain_id = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;
    let block_height = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;
    let raw_amount = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;
    let amount = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;

    let mut sender = [0u8; 32];
    sender.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    let mut receiver = [0u8; 32];
    receiver.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    let nonce = u64::from_le_bytes(body[offset..offset + 8].try_into()?);

    Ok(BridgeEventData {
        source_contract,
        target_contract,
        source_chain_id,
        target_chain_id,
        block_height,
        raw_amount,
        amount,
        sender,
        receiver,
        nonce,
    })
}

/// 一条 SVM 源链上桥合约的身份。
///
/// 事件只认**这个程序自己**打出的日志（#1133），且事件自报的来源必须就是它：
/// `source_contract == program_id`、`source_chain_id == chain_id`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SvmSource {
    pub program_id: Pubkey,
    pub chain_id: u64,
    pub kind: SvmProgramKind,
}

/// 从一笔交易的日志消息中提取桥合约**自己**发出的所有 Staked。
///
/// Anchor `emit!` 走 `sol_log_data`，日志形如 "Program data: {base64}"；
/// 但同一笔交易里任何程序都能打出同样格式、同样判别符的行（#1133）。
/// 所以先按运行时调用栈把 data 行归属到发出程序，只解析栈顶 == 桥合约的行；
/// 其它程序打出的 Staked 布局行按"疑似伪造"告警并丢弃。
///
/// 一笔交易可能包含多个事件（如批量操作），所以返回 Vec。
/// `Err(reason)`：日志无法可信归属或事件自报来源不符，整笔不采信。
fn extract_events_from_logs(
    logs: &[String],
    src: &SvmSource,
) -> std::result::Result<Vec<BridgeEventData>, &'static str> {
    let b64_engine = base64::engine::general_purpose::STANDARD;
    let attributed = attribute_program_data(logs, &src.program_id).map_err(|e| {
        warn!(chain_id = src.chain_id, "SVM 交易日志无法归属，整笔不采信: {e}");
        e.reason()
    })?;

    for (emitter, data_str) in &attributed.foreign {
        let Ok(data) = b64_engine.decode(data_str) else { continue };
        if data.len() >= 8 && data[..8] == staked_discriminator() {
            // 桥程序之外的程序打出 Staked 布局 —— 伪造尝试，或者是别的合约恰好
            // 也叫 Staked。两种都不能中继；前者必须让人看见。
            error!(
                alert = "BRIDGE_FORGED_STAKED_LOG",
                chain_id = src.chain_id,
                bridge_program = %src.program_id,
                emitter = %emitter,
                "非桥程序在同一笔交易里打出 Staked 布局日志，已丢弃"
            );
        }
    }

    let mut events = Vec::new();
    for data_str in attributed.own {
        let Ok(data) = b64_engine.decode(data_str) else { continue };
        // 鉴别器不匹配（桥合约的其它事件）会自动跳过
        let Ok(event) = parse_staked_from_data(&data) else { continue };
        if event.source_contract != src.program_id.to_bytes()
            || event.source_chain_id != src.chain_id
        {
            error!(
                alert = "BRIDGE_STAKED_SOURCE_MISMATCH",
                chain_id = src.chain_id,
                event_source_chain_id = event.source_chain_id,
                event_source_contract = %Pubkey::new_from_array(event.source_contract),
                nonce = event.nonce,
                "桥程序发出的 Staked 自报来源与所在链/程序不符，整笔不采信"
            );
            return Err("event-source-mismatch");
        }
        events.push(event);
    }

    Ok(events)
}

/// 获取程序当前最新一条已 finalized 的交易签名。
///
/// 用于首次启动时初始化 checkpoint：把"现在"作为起点，避免历史交易被误重放。
/// 如果程序当前还没有任何交易，返回 `Ok(None)` —— 此时也无需 checkpoint，
/// 后续第一笔交易就是起点。
///
/// 与 `poll_svm_events` 不同，这里只拉一条签名，不拉交易详情，开销很低。
pub async fn head_signature(rpc: &RpcClient, program_id: &Pubkey) -> Result<Option<Signature>> {
    use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;

    let config = GetConfirmedSignaturesForAddress2Config {
        before: None,
        until: None,
        limit: Some(1),
        commitment: Some(CommitmentConfig::finalized()),
    };

    let batch = rpc
        .get_signatures_for_address_with_config(program_id, config)
        .await
        .context("getSignaturesForAddress(limit=1) 调用失败")?;

    if batch.is_empty() {
        return Ok(None);
    }

    let sig = batch[0]
        .signature
        .parse::<Signature>()
        .context("解析 head 签名失败")?;
    Ok(Some(sig))
}

/// 枚举从 `until_sig` 之后到当前最新的**全部** finalized 签名。
///
/// 只做分页 `getSignaturesForAddress`，**不调 getTransaction**。
/// 返回按从旧到新排列的签名列表（自动反转 Solana 的新→旧顺序）。
///
/// **不做总量截断**：每轮从链头一路翻页到 `until_sig`（checkpoint）为止。
/// 这是不丢事件的关键 —— 之前的版本一旦积压超过某个上限就 `truncate` 保留
/// 「最新 N 个」，而调用方又把 checkpoint 推进到最新，导致 checkpoint 与旧
/// 积压之间那截签名永久落到 `until` 后面、再也拉不回来（长停机重启 + 高交易
/// 量时静默丢事件）。全量翻页保证返回列表与 checkpoint 连续，绝不留缺口。
///
/// 签名信息（sig-info）本身很轻（仅签名 + slot + err，不含交易体），即便积压
/// 上万，也就是几百次 `batch_size` 分页调用；真正昂贵的 `getTransaction` 由
/// 下游 extractor 按自身节奏从磁盘队列消化，不受此处影响。
///
/// 用于 SVM sig enumerator task：拿到 sig 列表后逐个 `save_new_sig` 写磁盘，
/// 再推进 checkpoint 到最新的 sig。
pub async fn enumerate_new_signatures(
    rpc: &RpcClient,
    program_id: &Pubkey,
    until_sig: Option<&Signature>,
    batch_size: usize,
) -> Result<Vec<Signature>> {
    use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;

    let mut all_sig_infos = Vec::new();
    let mut before_cursor: Option<Signature> = None;

    loop {
        let config = GetConfirmedSignaturesForAddress2Config {
            before: before_cursor,
            until: until_sig.copied(),
            limit: Some(batch_size),
            commitment: Some(CommitmentConfig::finalized()),
        };

        let batch = rpc
            .get_signatures_for_address_with_config(program_id, config)
            .await
            .context("getSignaturesForAddress 调用失败")?;

        let batch_len = batch.len();

        if batch.is_empty() {
            break;
        }

        let oldest_sig: Signature = batch
            .last()
            .unwrap()
            .signature
            .parse()
            .context("解析本页最老签名失败")?;

        all_sig_infos.extend(batch);

        // batch 不满一页 → 已翻到 until_sig（或链头），结束。
        if batch_len < batch_size {
            break;
        }

        before_cursor = Some(oldest_sig);

        debug!(
            fetched = all_sig_infos.len(),
            "正在分页获取 getSignaturesForAddress（全量翻到 checkpoint）"
        );
    }

    let mut sigs = Vec::with_capacity(all_sig_infos.len());
    for info in all_sig_infos.iter().rev() {
        if info.err.is_some() {
            continue;
        }
        let sig: Signature = info.signature.parse().context("解析交易签名失败")?;
        sigs.push(sig);
    }
    Ok(sigs)
}

/// 拉取单笔 sig 的交易详情并提取 Staked。
///
/// 三种"拿不到 logs"路径（RPC Err / meta=None / log_messages=None）
/// 按 H0 语义统一返回 `Err`，由调用方决定重试策略。
///
/// 每个提取出的 Staked 还要过链上 `StakeRecord` 佐证（#1133）：日志只是
/// 线索，账户状态才是证据。佐证失败同样返回 `Err` —— 重试耗尽后转 DLQ，
/// 由人核查，绝不在证据不足时投票。
pub async fn fetch_and_extract_events(
    rpc: &RpcClient,
    sig: &Signature,
    src: &SvmSource,
) -> Result<Vec<BridgeEventData>> {
    let tx_config = RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::Json),
        commitment: Some(CommitmentConfig::finalized()),
        max_supported_transaction_version: Some(0),
    };

    let tx_response = rpc
        .get_transaction_with_config(sig, tx_config)
        .await
        .with_context(|| format!("getTransaction({sig}) 调用失败"))?;

    // 枚举阶段已按 sig-info 的 err 跳过失败交易；这里按交易本体再判一次，
    // 失败交易的一切状态都被回滚，它的日志不代表任何已发生的事实。
    if tx_response
        .transaction
        .meta
        .as_ref()
        .is_some_and(|meta| meta.err.is_some())
    {
        debug!(tx = %sig, "交易执行失败，忽略其日志");
        return Ok(Vec::new());
    }

    let logs_tri = tx_response
        .transaction
        .meta
        .as_ref()
        .map(|meta| Option::<&Vec<String>>::from(meta.log_messages.as_ref()));

    match classify_tx_logs(logs_tri, src) {
        SigLogsOutcome::Events(events) => {
            for event in &events {
                verify_staked_against_chain(rpc, src.kind, &src.program_id, event)
                    .await
                    .with_context(|| format!("tx {sig} 的 Staked 未通过链上佐证"))?;
                debug!(
                    nonce = event.nonce,
                    amount = event.amount,
                    tx = %sig,
                    "解析到 SVM Staked（已过 StakeRecord 佐证）"
                );
            }
            Ok(events)
        }
        SigLogsOutcome::Unfetchable(reason) => {
            anyhow::bail!("getTransaction({sig}) 返回缺失关键字段: {reason}");
        }
    }
}

/// 单条 SVM 签名经过 `getTransaction` 之后，根据 logs 字段三态判定的下一步动作。
///
/// 三态来源：
/// - `None`：`tx.meta` 整个为 `None`（节点剪枝 / disable-rpc-transaction-history）
/// - `Some(None)`：`tx.meta.log_messages` 为 `None`（部分节点为省带宽裁掉了 logs）
/// - `Some(Some(logs))`：拿到完整 logs，正常解析
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SigLogsOutcome {
    /// 拿到 logs 并已解析（可能 0 个事件，也属正常路径）
    Events(Vec<BridgeEventData>),
    /// 关键字段缺失，或日志无法可信归属（#1133：截断 / 调用栈不自洽 /
    /// 事件自报来源不符），应该按 fetch failure 处理：本轮不推进 checkpoint。
    /// 内部的 `&'static str` 用于日志，不参与逻辑判断。
    Unfetchable(&'static str),
}

/// 把 meta + log_messages 的三态折叠成 `SigLogsOutcome`。
///
/// 提取成纯函数主要为了**单测可达**：直接测 `fetch_and_extract_events` 需要
/// mock 整个 `RpcClient`，而这部分判定逻辑才是新加防御代码的核心。
pub(crate) fn classify_tx_logs(
    logs_tri: Option<Option<&Vec<String>>>,
    src: &SvmSource,
) -> SigLogsOutcome {
    match logs_tri {
        None => SigLogsOutcome::Unfetchable("meta-none"),
        Some(None) => SigLogsOutcome::Unfetchable("log_messages-none"),
        Some(Some(logs)) => match extract_events_from_logs(logs, src) {
            Ok(events) => SigLogsOutcome::Events(events),
            Err(reason) => SigLogsOutcome::Unfetchable(reason),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge_program() -> Pubkey {
        Pubkey::new_from_array([0x01; 32])
    }

    fn src() -> SvmSource {
        SvmSource {
            program_id: bridge_program(),
            chain_id: 91024,
            kind: SvmProgramKind::Hub,
        }
    }

    fn staked_b64(event: &BridgeEventData) -> String {
        use base64::Engine;
        let body = borsh::to_vec(event).unwrap();
        let mut data = Vec::with_capacity(8 + body.len());
        data.extend_from_slice(&staked_discriminator());
        data.extend_from_slice(&body);
        base64::engine::general_purpose::STANDARD.encode(&data)
    }

    /// 用运行时的调用栈行把 `inner` 包进 `program` 的一帧（depth 1）。
    fn framed(program: &Pubkey, inner: Vec<String>) -> Vec<String> {
        let mut logs = vec![format!("Program {program} invoke [1]")];
        logs.extend(inner);
        logs.push(format!("Program {program} success"));
        logs
    }

    fn sample_event() -> BridgeEventData {
        BridgeEventData {
            source_contract: [0x01; 32],
            target_contract: [0x02; 32],
            source_chain_id: 91024,
            target_chain_id: 1,
            block_height: 100,
            raw_amount: 999,
            amount: 999,
            sender: [0x03; 32],
            receiver: [0x04; 32],
            nonce: 5,
        }
    }

    /// 鉴别器是 SHA256("event:Staked")[..8]，与合约 emit 时的 anchor 行为一致。
    #[test]
    fn staked_discriminator_matches_anchor_formula() {
        let mut hasher = Sha256::new();
        hasher.update("event:Staked");
        let expected = &hasher.finalize()[..8];
        let got = staked_discriminator();
        assert_eq!(&got[..], expected);
    }

    /// 把一个 BridgeEventData 用 borsh 序列化再加上鉴别器头，
    /// parse 出来的应该和原始数据完全一致 —— 这条 round-trip
    /// 验证 SVM 端事件解析正确。
    #[test]
    fn parse_staked_borsh_roundtrip() {
        let original = sample_event();
        let body = borsh::to_vec(&original).expect("serialize");
        let mut data = Vec::with_capacity(8 + body.len());
        data.extend_from_slice(&staked_discriminator());
        data.extend_from_slice(&body);

        let parsed = parse_staked_from_data(&data).expect("parse ok");
        assert_eq!(parsed, original);
    }

    /// 鉴别器不匹配 → 返回 Err（而不是误把别的 event 当成 Staked）。
    #[test]
    fn parse_staked_rejects_wrong_discriminator() {
        let body = borsh::to_vec(&sample_event()).unwrap();
        let mut data = Vec::with_capacity(8 + body.len());
        data.extend_from_slice(&[0xff; 8]); // 错的鉴别器
        data.extend_from_slice(&body);
        assert!(parse_staked_from_data(&data).is_err());
    }

    /// 数据不足 8 + BORSH_LEN 字节 → Err，而不是越界 panic。
    #[test]
    fn parse_staked_rejects_short_data() {
        let mut data = vec![0u8; 8 + BridgeEventData::BORSH_LEN - 1];
        data[..8].copy_from_slice(&staked_discriminator());
        assert!(parse_staked_from_data(&data).is_err());
    }

    /// extract_events_from_logs 应能从混合日志里挑出 Staked，忽略其它行。
    #[test]
    fn extract_events_from_logs_filters_out_unrelated_lines() {
        let event = sample_event();
        let logs = framed(
            &bridge_program(),
            vec![
                "Program log: hello".to_string(),
                format!("Program data: {}", staked_b64(&event)),
                "Program log: end".to_string(),
            ],
        );
        let parsed = extract_events_from_logs(&logs, &src()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], event);
    }

    // ─────────────────────────────────────────────────────────────────────
    // #1133：伪造 Staked 日志
    // ─────────────────────────────────────────────────────────────────────

    /// 攻击者程序与桥合约同处一笔交易（让 tx 进入桥程序的签名索引），
    /// 在自己的帧里打一行字段自洽的 Staked —— 必须被丢弃。
    #[test]
    fn staked_emitted_by_other_program_is_not_relayed() {
        let attacker = Pubkey::new_from_array([0xAA; 32]);
        let forged = sample_event();
        let mut logs = framed(&bridge_program(), vec!["Program log: noop".to_string()]);
        logs.extend(framed(
            &attacker,
            vec![format!("Program data: {}", staked_b64(&forged))],
        ));
        assert!(extract_events_from_logs(&logs, &src()).unwrap().is_empty());
    }

    /// 没有任何调用栈行、只有一行 data（旧解析器会照单全收）→ 整笔不采信。
    #[test]
    fn bare_data_line_without_frame_is_rejected() {
        let logs = vec![format!("Program data: {}", staked_b64(&sample_event()))];
        assert_eq!(
            extract_events_from_logs(&logs, &src()),
            Err("log-stack-malformed")
        );
    }

    /// 桥程序自己发出、但自报来源是别的链 / 别的合约 → 整笔不采信。
    #[test]
    fn staked_with_foreign_source_fields_is_rejected() {
        let mut ev = sample_event();
        ev.source_chain_id = 103;
        let logs = framed(
            &bridge_program(),
            vec![format!("Program data: {}", staked_b64(&ev))],
        );
        assert_eq!(
            extract_events_from_logs(&logs, &src()),
            Err("event-source-mismatch")
        );

        let mut ev = sample_event();
        ev.source_contract = [0x77; 32];
        let logs = framed(
            &bridge_program(),
            vec![format!("Program data: {}", staked_b64(&ev))],
        );
        assert_eq!(
            extract_events_from_logs(&logs, &src()),
            Err("event-source-mismatch")
        );
    }

    /// 日志被截断 → 截断后的事件不可见，不能当作"没有事件"删掉 sig。
    #[test]
    fn truncated_logs_are_unfetchable() {
        let mut logs = framed(
            &bridge_program(),
            vec![format!("Program data: {}", staked_b64(&sample_event()))],
        );
        logs.push("Log truncated".to_string());
        assert_eq!(
            classify_tx_logs(Some(Some(&logs)), &src()),
            SigLogsOutcome::Unfetchable("log-truncated")
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // C2 修复：classify_tx_logs 三态判定
    // 防御性单测，确保任意一条新增的失败子情形不会被误归为 Events，
    // 否则就会再次出现"checkpoint 跨过失败 sig，事件静默丢失"的问题。
    // ─────────────────────────────────────────────────────────────────────

    /// `meta == None`（节点剪枝 transaction history） → 必须返回 Unfetchable，
    /// 调用方据此**不推进 checkpoint**。
    #[test]
    fn classify_tx_logs_meta_none_is_unfetchable() {
        match classify_tx_logs(None, &src()) {
            SigLogsOutcome::Unfetchable(reason) => assert_eq!(reason, "meta-none"),
            other => panic!("expected Unfetchable(meta-none), got {other:?}"),
        }
    }

    /// `meta` 存在但 `log_messages == None`（节点裁掉 logs） → 必须返回 Unfetchable。
    #[test]
    fn classify_tx_logs_log_messages_none_is_unfetchable() {
        match classify_tx_logs(Some(None), &src()) {
            SigLogsOutcome::Unfetchable(reason) => assert_eq!(reason, "log_messages-none"),
            other => panic!("expected Unfetchable(log_messages-none), got {other:?}"),
        }
    }

    /// `logs == Some(vec![])` 是合法 case（虽然桥合约 tx 实际不会出现），
    /// 必须走 Events 路径，**不能**误判为 Unfetchable，否则会卡死 checkpoint。
    #[test]
    fn classify_tx_logs_empty_logs_is_events_not_unfetchable() {
        let empty: Vec<String> = vec![];
        match classify_tx_logs(Some(Some(&empty)), &src()) {
            SigLogsOutcome::Events(events) => assert!(events.is_empty()),
            other => panic!("expected Events([]), got {other:?}"),
        }
    }

    /// 拿到完整 logs 且其中含 Staked → Events(events)。
    #[test]
    fn classify_tx_logs_with_staked_returns_events() {
        let event = sample_event();
        let logs = framed(
            &bridge_program(),
            vec![
                "Program log: hello".to_string(),
                format!("Program data: {}", staked_b64(&event)),
            ],
        );
        match classify_tx_logs(Some(Some(&logs)), &src()) {
            SigLogsOutcome::Events(events) => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0], event);
            }
            other => panic!("expected Events([event]), got {other:?}"),
        }
    }

    /// 拿到完整 logs 但里面没 Staked（只有无关的 Anchor boilerplate） → Events([])。
    /// 这是常见 case：调桥合约的 init / configure / pause 等指令不发 Staked。
    #[test]
    fn classify_tx_logs_unrelated_logs_returns_empty_events() {
        let logs = framed(
            &bridge_program(),
            vec!["Program log: configure complete".to_string()],
        );
        match classify_tx_logs(Some(Some(&logs)), &src()) {
            SigLogsOutcome::Events(events) => assert!(events.is_empty()),
            other => panic!("expected Events([]), got {other:?}"),
        }
    }
}

/// 真链回放（#1133）：对一条 SVM 链上桥程序的历史签名逐笔跑「归属 + StakeRecord 佐证」，
/// 确认修复不会误杀真实 stake。只读，默认不跑：
///
/// ```text
/// BRIDGE_LIVE_SVM_RPC=https://api.devnet.solana.com \
/// BRIDGE_LIVE_SVM_PROGRAM=7mXG6UYSDbo1yC11Hcm4C7ptWM4YppLcDqjSkM5dfihc \
/// BRIDGE_LIVE_SVM_CHAIN_ID=103 BRIDGE_LIVE_SVM_KIND=leaf \
/// cargo test --release live_history -- --ignored --nocapture
/// ```
#[cfg(test)]
mod live_replay {
    use super::*;
    use std::str::FromStr;

    #[tokio::test]
    #[ignore]
    async fn live_history_passes_attribution_and_proof() {
        let rpc_url = std::env::var("BRIDGE_LIVE_SVM_RPC").expect("BRIDGE_LIVE_SVM_RPC");
        let program_id =
            Pubkey::from_str(&std::env::var("BRIDGE_LIVE_SVM_PROGRAM").expect("PROGRAM")).unwrap();
        let chain_id: u64 = std::env::var("BRIDGE_LIVE_SVM_CHAIN_ID").expect("CHAIN_ID").parse().unwrap();
        let kind = match std::env::var("BRIDGE_LIVE_SVM_KIND").as_deref() {
            Ok("hub") => SvmProgramKind::Hub,
            _ => SvmProgramKind::Leaf,
        };
        let src = SvmSource { program_id, chain_id, kind };
        let rpc = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::finalized());

        let sigs = enumerate_new_signatures(&rpc, &program_id, None, 1000).await.unwrap();
        let (mut staked, mut failed) = (0usize, Vec::new());
        for sig in &sigs {
            match fetch_and_extract_events(&rpc, sig, &src).await {
                Ok(events) => {
                    for ev in &events {
                        println!("OK  {sig} nonce={} raw={} net={}", ev.nonce, ev.raw_amount, ev.amount);
                    }
                    staked += events.len();
                }
                Err(e) => {
                    println!("ERR {sig}: {e:#}");
                    failed.push(*sig);
                }
            }
        }
        println!("signatures={} staked_verified={} rejected={}", sigs.len(), staked, failed.len());
        assert!(failed.is_empty(), "真实历史交易被误杀: {failed:?}");
    }
}
