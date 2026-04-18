//! SVM 事件轮询模块
//!
//! 通过 Solana RPC 的 getSignaturesForAddress 接口获取桥合约的交易签名，
//! 然后逐笔拉取交易日志，从中解析 Anchor 格式的 StakeEvent。
//!
//! 核心特点：
//! - 分页获取签名（batch_size 控制每页大小，max_total 控制总量上限）
//! - 使用 finalized 确认级别，只处理已最终确认的交易
//! - 自动识别 Anchor 的 "Program data:" 日志前缀和事件鉴别器

use anyhow::{Context, Result};
use base64::Engine;
use sha2::{Digest, Sha256};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_transaction_status::UiTransactionEncoding;
use tracing::{debug, warn};

use crate::types::StakeEventData;

/// 计算 Anchor 事件的鉴别器。
/// Anchor 约定：SHA-256("event:{事件名}") 的前 8 字节。
fn stake_event_discriminator() -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update("event:StakeEvent");
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

/// 从 Anchor 程序日志数据中解析 StakeEvent。
///
/// 数据布局：
/// ```text
/// [8B 鉴别器] [Borsh 序列化的 StakeEventData (168B)]
/// ```
///
/// StakeEventData 的 Borsh 布局（小端序）：
/// - source_contract: [u8; 32]
/// - target_contract: [u8; 32]
/// - source_chain_id: u64 (8B LE)
/// - target_chain_id: u64
/// - block_height: u64
/// - amount: u64
/// - sender: [u8; 32]
/// - receiver: [u8; 32]
/// - nonce: u64
fn parse_stake_event_from_data(data: &[u8]) -> Result<StakeEventData> {
    let disc = stake_event_discriminator();
    if data.len() < 8 + StakeEventData::BORSH_LEN {
        anyhow::bail!("StakeEvent 数据太短: {} 字节", data.len());
    }
    // 检查鉴别器是否匹配
    if data[..8] != disc {
        anyhow::bail!("不是 StakeEvent（鉴别器不匹配）");
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
    let amount = u64::from_le_bytes(body[offset..offset + 8].try_into()?);
    offset += 8;

    let mut sender = [0u8; 32];
    sender.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    let mut receiver = [0u8; 32];
    receiver.copy_from_slice(&body[offset..offset + 32]);
    offset += 32;

    let nonce = u64::from_le_bytes(body[offset..offset + 8].try_into()?);

    Ok(StakeEventData {
        source_contract,
        target_contract,
        source_chain_id,
        target_chain_id,
        block_height,
        amount,
        sender,
        receiver,
        nonce,
    })
}

/// 从一笔交易的日志消息中提取所有 StakeEvent。
///
/// Anchor 程序通过 `msg!` 输出日志，事件数据以 "Program data: {base64}" 的格式记录。
/// 一笔交易可能包含多个事件（如批量操作），所以返回 Vec。
fn extract_events_from_logs(logs: &[String]) -> Vec<StakeEventData> {
    let b64_engine = base64::engine::general_purpose::STANDARD;
    let mut events = Vec::new();

    for log_line in logs {
        // Anchor 事件日志的固定前缀
        if let Some(data_str) = log_line.strip_prefix("Program data: ") {
            // 尝试 base64 解码
            if let Ok(data) = b64_engine.decode(data_str.trim()) {
                // 尝试解析为 StakeEvent（鉴别器不匹配会自动跳过）
                if let Ok(event) = parse_stake_event_from_data(&data) {
                    events.push(event);
                }
            }
        }
    }

    events
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

/// 轮询 SVM 链上的 StakeEvent 事件，支持自动分页。
///
/// 工作流程：
/// 1. 通过 getSignaturesForAddress 分页获取交易签名（从新到旧）
/// 2. 对每笔成功的交易，拉取完整交易内容并解析日志
/// 3. 从日志中提取 StakeEvent 事件
///
/// 参数：
/// - `program_id`：桥合约的 Program ID
/// - `until_sig`：上次扫描到的最新签名（作为停止边界，exclusive）
/// - `batch_size`：每页获取的签名数量（如 50）
/// - `max_total`：单轮最多累计获取的签名数量（如 1000）
///
/// 返回：
/// - `(events, newest_signature)`：事件列表（按时间从旧到新排列）和本轮最新的签名
///
/// 分页策略：
/// getSignaturesForAddress 返回从新到旧的签名列表，通过 `before` 游标逐页向历史回溯。
/// 如果某页返回的数量小于 batch_size，说明已经没有更多数据了。
pub async fn poll_svm_events(
    rpc: &RpcClient,
    program_id: &Pubkey,
    until_sig: Option<&Signature>,
    batch_size: usize,
    max_total: usize,
) -> Result<(Vec<StakeEventData>, Option<Signature>)> {
    use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;

    let mut all_sig_infos = Vec::new();
    let mut before_cursor: Option<Signature> = None; // 分页游标

    // ── 分页获取签名 ──
    loop {
        let config = GetConfirmedSignaturesForAddress2Config {
            before: before_cursor,          // 从此签名之前开始（不含）
            until: until_sig.copied(),      // 到 checkpoint 签名为止（不含）
            limit: Some(batch_size),        // 每页大小
            commitment: Some(CommitmentConfig::finalized()),
        };

        let batch = rpc
            .get_signatures_for_address_with_config(program_id, config)
            .await
            .context("getSignaturesForAddress 调用失败")?;

        let batch_len = batch.len();

        // 空页说明没有更多签名
        if batch.is_empty() {
            break;
        }

        // 记录本页最老的签名，用作下一页的 before 游标
        let oldest_sig: Signature = batch
            .last()
            .unwrap()
            .signature
            .parse()
            .context("解析本页最老签名失败")?;

        all_sig_infos.extend(batch);

        // 达到总量上限，截断
        if all_sig_infos.len() >= max_total {
            all_sig_infos.truncate(max_total);
            break;
        }

        // 如果本页不满，说明已经到头了
        if batch_len < batch_size {
            break;
        }

        // 移动游标到本页最老的签名
        before_cursor = Some(oldest_sig);

        debug!(
            fetched = all_sig_infos.len(),
            max_total,
            "正在分页获取 getSignaturesForAddress"
        );
    }

    if all_sig_infos.is_empty() {
        return Ok((vec![], None));
    }

    // 记录本轮最新的签名（第一条，因为 getSignaturesForAddress 按从新到旧排列）
    let newest_sig = all_sig_infos[0]
        .signature
        .parse::<Signature>()
        .context("解析最新签名失败")?;

    debug!(
        total_sigs = all_sig_infos.len(),
        "开始逐笔获取交易详情"
    );

    let mut all_events = Vec::new();
    // 任何一笔签名拉不到完整 logs（Err / meta=None / log_messages=None）都标记此 flag。
    // 只要为 true，本轮就不返回 newest_sig，调用方据此**不推进 checkpoint**，
    // 下一轮同样区间会被 getSignaturesForAddress 重新枚举出来重试一次。
    //
    // 这是必须的：getSignaturesForAddress 返回的所有签名都调用过桥合约，
    // 任何一笔的 logs 取不到，就意味着我们**不知道里面有没有 StakeEvent**，
    // 直接放过去 = 静默丢账。具体三种失败模式见 docs/audit-log.md "C2"。
    let mut had_fetch_failure = false;

    // 从旧到新遍历签名（反转顺序），确保事件按时间顺序排列
    for sig_info in all_sig_infos.iter().rev() {
        // 跳过失败的交易（err 字段非空）
        if sig_info.err.is_some() {
            continue;
        }

        let sig: Signature = sig_info
            .signature
            .parse()
            .context("解析交易签名失败")?;

        // 拉取交易详情（包含日志）
        let tx_config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::Json),
            commitment: Some(CommitmentConfig::finalized()),
            max_supported_transaction_version: Some(0),
        };

        match rpc.get_transaction_with_config(&sig, tx_config).await {
            Ok(tx_response) => {
                // 把 meta + log_messages 的三态折叠成 SigLogsOutcome，便于测试与日志分类
                let logs_tri = tx_response
                    .transaction
                    .meta
                    .as_ref()
                    .map(|meta| Option::<&Vec<String>>::from(meta.log_messages.as_ref()));

                match classify_tx_logs(logs_tri) {
                    SigLogsOutcome::Events(events) => {
                        for event in events {
                            debug!(
                                nonce = event.nonce,
                                amount = event.amount,
                                tx = %sig,
                                "解析到 SVM StakeEvent"
                            );
                            all_events.push(event);
                        }
                    }
                    SigLogsOutcome::Unfetchable(reason) => {
                        warn!(
                            tx = %sig,
                            reason,
                            "getTransaction 返回缺失关键字段，本轮不推进 checkpoint，下一轮重试"
                        );
                        had_fetch_failure = true;
                    }
                }
            }
            // 网络抖动 / RPC 超时 / 节点临时故障 —— 按 fetch failure 处理。
            Err(e) => {
                warn!(
                    tx = %sig,
                    "getTransaction 调用失败，本轮不推进 checkpoint，下一轮重试: {e}"
                );
                had_fetch_failure = true;
            }
        }
    }

    // 任意一笔失败 → 返回 None 给调用方，告知"本轮不推进 checkpoint"。
    // 下一轮 until_sig 不变，failed sig 会被 getSignaturesForAddress 再次枚举出来。
    let advance_to = if had_fetch_failure {
        None
    } else {
        Some(newest_sig)
    };
    Ok((all_events, advance_to))
}

/// 单条 SVM 签名经过 `getTransaction` 之后，根据 logs 字段三态判定的下一步动作。
///
/// 三态来源：
/// - `None`：`tx.meta` 整个为 `None`（节点剪枝 / disable-rpc-transaction-history）
/// - `Some(None)`：`tx.meta.log_messages` 为 `None`（部分节点为省带宽裁掉了 logs）
/// - `Some(Some(logs))`：拿到完整 logs，正常解析
#[derive(Debug, PartialEq, Eq)]
enum SigLogsOutcome {
    /// 拿到 logs 并已解析（可能 0 个事件，也属正常路径）
    Events(Vec<StakeEventData>),
    /// 关键字段缺失，应该按 fetch failure 处理：本轮不推进 checkpoint。
    /// 内部的 `&'static str` 用于日志，不参与逻辑判断。
    Unfetchable(&'static str),
}

/// 把 meta + log_messages 的三态折叠成 `SigLogsOutcome`。
///
/// 提取成纯函数主要为了**单测可达**：直接测 `poll_svm_events` 需要 mock 整个
/// `RpcClient`，而这部分判定逻辑才是新加防御代码的核心。
fn classify_tx_logs(logs_tri: Option<Option<&Vec<String>>>) -> SigLogsOutcome {
    match logs_tri {
        None => SigLogsOutcome::Unfetchable("meta-none"),
        Some(None) => SigLogsOutcome::Unfetchable("log_messages-none"),
        Some(Some(logs)) => SigLogsOutcome::Events(extract_events_from_logs(logs)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> StakeEventData {
        StakeEventData {
            source_contract: [0x01; 32],
            target_contract: [0x02; 32],
            source_chain_id: 91024,
            target_chain_id: 1,
            block_height: 100,
            amount: 999,
            sender: [0x03; 32],
            receiver: [0x04; 32],
            nonce: 5,
        }
    }

    /// 鉴别器是 SHA256("event:StakeEvent")[..8]，与合约 emit 时的 anchor 行为一致。
    #[test]
    fn stake_event_discriminator_matches_anchor_formula() {
        let mut hasher = Sha256::new();
        hasher.update("event:StakeEvent");
        let expected = &hasher.finalize()[..8];
        let got = stake_event_discriminator();
        assert_eq!(&got[..], expected);
    }

    /// 把一个 StakeEventData 用 borsh 序列化再加上鉴别器头，
    /// parse 出来的应该和原始数据完全一致 —— 这条 round-trip
    /// 验证 SVM 端事件解析正确。
    #[test]
    fn parse_stake_event_borsh_roundtrip() {
        let original = sample_event();
        let body = borsh::to_vec(&original).expect("serialize");
        let mut data = Vec::with_capacity(8 + body.len());
        data.extend_from_slice(&stake_event_discriminator());
        data.extend_from_slice(&body);

        let parsed = parse_stake_event_from_data(&data).expect("parse ok");
        assert_eq!(parsed, original);
    }

    /// 鉴别器不匹配 → 返回 Err（而不是误把别的 event 当成 StakeEvent）。
    #[test]
    fn parse_stake_event_rejects_wrong_discriminator() {
        let body = borsh::to_vec(&sample_event()).unwrap();
        let mut data = Vec::with_capacity(8 + body.len());
        data.extend_from_slice(&[0xff; 8]); // 错的鉴别器
        data.extend_from_slice(&body);
        assert!(parse_stake_event_from_data(&data).is_err());
    }

    /// 数据不足 8 + BORSH_LEN 字节 → Err，而不是越界 panic。
    #[test]
    fn parse_stake_event_rejects_short_data() {
        let mut data = vec![0u8; 8 + StakeEventData::BORSH_LEN - 1];
        data[..8].copy_from_slice(&stake_event_discriminator());
        assert!(parse_stake_event_from_data(&data).is_err());
    }

    /// extract_events_from_logs 应能从混合日志里挑出 StakeEvent，忽略其它行。
    #[test]
    fn extract_events_from_logs_filters_out_unrelated_lines() {
        use base64::Engine;

        let event = sample_event();
        let body = borsh::to_vec(&event).unwrap();
        let mut data = Vec::with_capacity(8 + body.len());
        data.extend_from_slice(&stake_event_discriminator());
        data.extend_from_slice(&body);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);

        let logs = vec![
            "Program log: hello".to_string(),
            format!("Program data: {b64}"),
            "Program log: end".to_string(),
        ];
        let parsed = extract_events_from_logs(&logs);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], event);
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
        match classify_tx_logs(None) {
            SigLogsOutcome::Unfetchable(reason) => assert_eq!(reason, "meta-none"),
            other => panic!("expected Unfetchable(meta-none), got {other:?}"),
        }
    }

    /// `meta` 存在但 `log_messages == None`（节点裁掉 logs） → 必须返回 Unfetchable。
    #[test]
    fn classify_tx_logs_log_messages_none_is_unfetchable() {
        match classify_tx_logs(Some(None)) {
            SigLogsOutcome::Unfetchable(reason) => assert_eq!(reason, "log_messages-none"),
            other => panic!("expected Unfetchable(log_messages-none), got {other:?}"),
        }
    }

    /// `logs == Some(vec![])` 是合法 case（虽然桥合约 tx 实际不会出现），
    /// 必须走 Events 路径，**不能**误判为 Unfetchable，否则会卡死 checkpoint。
    #[test]
    fn classify_tx_logs_empty_logs_is_events_not_unfetchable() {
        let empty: Vec<String> = vec![];
        match classify_tx_logs(Some(Some(&empty))) {
            SigLogsOutcome::Events(events) => assert!(events.is_empty()),
            other => panic!("expected Events([]), got {other:?}"),
        }
    }

    /// 拿到完整 logs 且其中含 StakeEvent → Events(events)。
    #[test]
    fn classify_tx_logs_with_stake_event_returns_events() {
        let event = sample_event();
        let body = borsh::to_vec(&event).unwrap();
        let mut data = Vec::with_capacity(8 + body.len());
        data.extend_from_slice(&stake_event_discriminator());
        data.extend_from_slice(&body);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);

        let logs = vec![
            "Program log: hello".to_string(),
            format!("Program data: {b64}"),
        ];
        match classify_tx_logs(Some(Some(&logs))) {
            SigLogsOutcome::Events(events) => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0], event);
            }
            other => panic!("expected Events([event]), got {other:?}"),
        }
    }

    /// 拿到完整 logs 但里面没 StakeEvent（只有无关的 Anchor boilerplate） → Events([])。
    /// 这是常见 case：调桥合约的 init / configure / pause 等指令不发 StakeEvent。
    #[test]
    fn classify_tx_logs_unrelated_logs_returns_empty_events() {
        let logs = vec![
            "Program FooBarBaz invoke [1]".to_string(),
            "Program log: configure complete".to_string(),
            "Program FooBarBaz success".to_string(),
        ];
        match classify_tx_logs(Some(Some(&logs))) {
            SigLogsOutcome::Events(events) => assert!(events.is_empty()),
            other => panic!("expected Events([]), got {other:?}"),
        }
    }
}
