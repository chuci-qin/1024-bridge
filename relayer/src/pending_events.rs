//! 待处理事件持久化模块
//!
//! 解决"内存 retry 队列在进程崩溃时丢失事件"的问题（审计 H1）。
//!
//! 核心思想：
//! - Poller（每条链 1 个）每次拉到新事件，先把 event 序列化写到磁盘文件，
//!   再推进 checkpoint。
//! - Submitter（每条链 1 个）每轮扫描自己 target 目录里的所有 pending 文件，
//!   状态机推进（详见下文）。
//! - 一个事件最终被 N confirmations 验证后才会删除文件。
//! - 任意一步失败则保留文件，下一轮再被扫到、再次重试。
//!
//! 文件状态机（pipelined submit + async confirmation）：
//! ```text
//! 1. NEW         : { event, submission: null }              ← poller 写入
//! 2. SUBMITTED   : { event, submission: { tx_hash, ... } }  ← submitter 广播后立即写入
//! 3. (deleted)                                              ← submitter 看到 N confs 后删除
//! ```
//! 关键不变量：
//! - submitter 的"广播"和"等确认"完全解耦：广播只等 RPC 接受 tx，立即写文件 → 处理下一笔；
//!   等确认是下一轮（甚至下下轮）通过 receipt 检查 + N 块确认完成的。
//! - 这样一个慢链上的 12 块确认（~2.4min）不会再阻塞同一 submitter 处理其它 100 个事件。
//!
//! 文件格式：`{ "event": StakeEventData, "submission": null|Submission }`
//!
//! 目录布局：`{events_root}/{target_chain_id}/{source_chain_id}_{nonce}.json`
//! - 路由由 `event.target_chain_id` 决定（写入端 = poller）。
//! - 消费由 `target_chain_id` 决定（读取端 = 该 target chain 的 submitter）。
//! - 文件名带 `source_chain_id` 前缀的原因：同一个 target 可能收到来自多条
//!   不同 source chain 的事件，nonce 是 per-source 的，不带 source 前缀会撞名。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::types::StakeEventData;

/// Submitter 已广播但尚未最终确认的状态记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    /// 链上 tx 标识。EVM：0x-前缀 hex；SVM：base58 signature。
    pub tx_hash: String,
    /// 广播 RPC 调用返回的 unix 秒时间戳；用于 stale 超时判断。
    pub sent_at_unix: u64,
    /// 第一次看到 receipt 后缓存的区块/slot 号；
    /// 缓存后的轮次只需要拉一次 latest_block，不再每轮重复拉 receipt。
    /// `None` 表示尚未看到回执。
    pub mined_block: Option<u64>,
}

/// 单个待处理事件的完整磁盘表示（事件本身 + 可选的提交状态）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEntry {
    pub event: StakeEventData,
    /// `None` = 尚未广播；`Some` = 已广播，等待 N confs 成熟。
    pub submission: Option<Submission>,
}

/// 当前 unix 秒时间戳。系统时钟反常时回退 0（不会 panic）。
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 构造某 target chain 的事件目录路径。
pub fn target_dir(events_root: &Path, target_chain_id: u64) -> PathBuf {
    events_root.join(target_chain_id.to_string())
}

/// 确保某 target chain 的事件目录存在，不存在则创建。返回该目录路径。
pub fn ensure_target_dir(events_root: &Path, target_chain_id: u64) -> Result<PathBuf> {
    let dir = target_dir(events_root, target_chain_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建事件目录失败: {}", dir.display()))?;
    Ok(dir)
}

/// 单个事件文件名：`{source_chain_id}_{nonce}.json`
fn event_filename(event: &StakeEventData) -> String {
    format!("{}_{}.json", event.source_chain_id, event.nonce)
}

/// 把新事件持久化（poller 写入路径）。
///
/// - 自动按 `event.target_chain_id` 路由，无需调用方传 dir。
/// - 自动 `mkdir -p` 目录，无需 caller 预先 ensure。
/// - 使用 `tmp + rename` 原子写策略。
/// - 同名文件已存在 → 直接返回 Ok（幂等：重复 poll 不会覆盖已有 submission 状态！）。
///
/// **重要**：此函数对已存在的文件 no-op，所以 submitter 写过 submission 之后，
/// poller 即使重复扫到同一事件也不会清空 submission 字段。
/// 要更新已有 entry 的 submission，使用 [`update_pending_entry`]。
pub fn save_pending_event(events_root: &Path, event: &StakeEventData) -> Result<()> {
    let dir = ensure_target_dir(events_root, event.target_chain_id)?;
    let path = dir.join(event_filename(event));
    if path.exists() {
        return Ok(());
    }
    let entry = PendingEntry { event: event.clone(), submission: None };
    write_entry_atomic(&path, &entry)
}

/// **覆盖**写入一个 entry（submitter 更新 submission 字段时用）。
///
/// 与 [`save_pending_event`] 的关键区别：此函数无条件覆盖已存在的文件，
/// 用 `tmp + rename` 保证读侧不会读到半截文件。
///
/// 路径仍由 `entry.event.target_chain_id` + 文件名规则确定，与 save 对齐。
pub fn update_pending_entry(events_root: &Path, entry: &PendingEntry) -> Result<()> {
    let dir = ensure_target_dir(events_root, entry.event.target_chain_id)?;
    let path = dir.join(event_filename(&entry.event));
    write_entry_atomic(&path, entry)
}

/// 内部：将 entry 原子写入指定路径（含 fsync）。
fn write_entry_atomic(path: &Path, entry: &PendingEntry) -> Result<()> {
    let json = serde_json::to_string_pretty(entry)?;
    crate::checkpoint::write_atomic_with_sync(path, json.as_bytes())
}

/// 删除已成功处理的事件文件。文件已不存在视为成功（幂等）。
pub fn delete_pending_event(events_root: &Path, event: &StakeEventData) -> Result<()> {
    let dir = target_dir(events_root, event.target_chain_id);
    let path = dir.join(event_filename(event));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("删除事件文件 {} 失败", path.display())),
    }
}

/// 扫描 `events_root/{target_chain_id}/` 目录，加载所有待处理 entry。
///
/// - 仅识别 `*.json` 文件，忽略 `*.json.tmp` 等中间文件。
/// - 文件名必须形如 `{source}_{nonce}.json`，否则 warn 跳过。
/// - 解析失败的文件会被跳过（warn 记录），不中断整体扫描。
/// - 返回 entry 按 `(source_chain_id, nonce)` 升序排列，使提交顺序稳定可预测。
///   多 source 同 target 的场景：源 1 的事件会先于源 2 的事件被尝试提交。
pub fn load_all_pending_events(
    events_root: &Path,
    target_chain_id: u64,
) -> Result<Vec<PendingEntry>> {
    let dir = target_dir(events_root, target_chain_id);
    let mut entries = Vec::new();
    if !dir.exists() {
        return Ok(entries);
    }

    let read_dir = std::fs::read_dir(&dir)
        .with_context(|| format!("读取事件目录失败: {}", dir.display()))?;

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("枚举事件目录条目失败: {e}");
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        // 校验文件名形如 {source}_{nonce}.json，避免读到陈旧的非法文件
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if parse_event_filename(stem).is_none() {
                tracing::warn!("跳过文件名不符合规范的事件文件: {}", path.display());
                continue;
            }
        }
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("读取事件文件 {} 失败: {e}", path.display());
                continue;
            }
        };
        match serde_json::from_str::<PendingEntry>(&data) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                tracing::warn!("解析事件文件 {} 失败: {e}", path.display());
            }
        }
    }

    entries.sort_by_key(|e| (e.event.source_chain_id, e.event.nonce));
    Ok(entries)
}

/// 解析 `{source_chain_id}_{nonce}` 形式的文件名 stem。
/// 返回 (source_chain_id, nonce)；格式不符返回 None。
fn parse_event_filename(stem: &str) -> Option<(u64, u64)> {
    let (src, nonce) = stem.split_once('_')?;
    let src: u64 = src.parse().ok()?;
    let nonce: u64 = nonce.parse().ok()?;
    Some((src, nonce))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_event(target: u64, source: u64, nonce: u64) -> StakeEventData {
        StakeEventData {
            source_contract: [0x11; 32],
            target_contract: [0x22; 32],
            source_chain_id: source,
            target_chain_id: target,
            block_height: 100,
            amount: 1_000_000,
            sender: [0x33; 32],
            receiver: [0x44; 32],
            nonce,
        }
    }

    fn empty_entry(target: u64, source: u64, nonce: u64) -> PendingEntry {
        PendingEntry {
            event: sample_event(target, source, nonce),
            submission: None,
        }
    }

    /// 基本 roundtrip：保存 → 加载 → 删除。
    #[test]
    fn save_load_delete_roundtrip() {
        let tmp = tempdir().unwrap();
        let ev = sample_event(91024, 1, 42);

        save_pending_event(tmp.path(), &ev).unwrap();
        let loaded = load_all_pending_events(tmp.path(), 91024).unwrap();
        assert_eq!(loaded, vec![empty_entry(91024, 1, 42)]);

        delete_pending_event(tmp.path(), &ev).unwrap();
        let after = load_all_pending_events(tmp.path(), 91024).unwrap();
        assert!(after.is_empty());
    }

    /// 不同 source chain 的事件 nonce 可以重叠，不会撞名（关键不变量）。
    /// e.g. Ethereum nonce=100 和 Arbitrum nonce=100 同时打到 1024。
    #[test]
    fn multi_source_same_nonce_no_collision() {
        let tmp = tempdir().unwrap();
        let ev_eth = sample_event(91024, 1, 100);
        let ev_arb = sample_event(91024, 42161, 100);

        save_pending_event(tmp.path(), &ev_eth).unwrap();
        save_pending_event(tmp.path(), &ev_arb).unwrap();

        let loaded = load_all_pending_events(tmp.path(), 91024).unwrap();
        assert_eq!(loaded.len(), 2);
        // 排序：(source_chain_id, nonce) 升序 → eth(1) 先于 arb(42161)
        assert_eq!(loaded[0].event, ev_eth);
        assert_eq!(loaded[1].event, ev_arb);
    }

    /// 不同 target chain 写到不同目录，互不干扰。
    #[test]
    fn different_target_chains_isolated() {
        let tmp = tempdir().unwrap();
        let to_eth = sample_event(1, 91024, 10);
        let to_arb = sample_event(42161, 91024, 11);

        save_pending_event(tmp.path(), &to_eth).unwrap();
        save_pending_event(tmp.path(), &to_arb).unwrap();

        assert_eq!(
            load_all_pending_events(tmp.path(), 1).unwrap(),
            vec![empty_entry(1, 91024, 10)]
        );
        assert_eq!(
            load_all_pending_events(tmp.path(), 42161).unwrap(),
            vec![empty_entry(42161, 91024, 11)]
        );
        // 第三条链从未写过 → 空
        assert!(load_all_pending_events(tmp.path(), 91024)
            .unwrap()
            .is_empty());
    }

    /// 重复 save 同一个事件应该幂等，不报错也不覆盖已有 submission。
    #[test]
    fn save_is_idempotent_and_preserves_submission() {
        let tmp = tempdir().unwrap();
        let ev = sample_event(91024, 1, 7);
        save_pending_event(tmp.path(), &ev).unwrap();

        // 模拟 submitter 已经写入了 submission
        let entry = PendingEntry {
            event: ev.clone(),
            submission: Some(Submission {
                tx_hash: "0xabc".into(),
                sent_at_unix: 1_700_000_000,
                mined_block: Some(123),
            }),
        };
        update_pending_entry(tmp.path(), &entry).unwrap();

        // poller 再次扫到同一事件，调用 save_pending_event：必须不覆盖 submission
        save_pending_event(tmp.path(), &ev).unwrap();

        let loaded = load_all_pending_events(tmp.path(), 91024).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].submission, entry.submission);
    }

    /// 删除不存在的文件不应报错（幂等）。
    #[test]
    fn delete_missing_is_ok() {
        let tmp = tempdir().unwrap();
        let ev = sample_event(91024, 1, 999);
        delete_pending_event(tmp.path(), &ev).unwrap();
    }

    /// 加载排序：按 (source, nonce) 升序，无论写入顺序如何。
    #[test]
    fn load_order_is_stable_by_source_then_nonce() {
        let tmp = tempdir().unwrap();
        // 故意逆序写
        save_pending_event(tmp.path(), &sample_event(91024, 42161, 5)).unwrap();
        save_pending_event(tmp.path(), &sample_event(91024, 1, 20)).unwrap();
        save_pending_event(tmp.path(), &sample_event(91024, 1, 3)).unwrap();
        save_pending_event(tmp.path(), &sample_event(91024, 42161, 1)).unwrap();

        let loaded = load_all_pending_events(tmp.path(), 91024).unwrap();
        assert_eq!(loaded.len(), 4);
        let keys: Vec<_> = loaded
            .iter()
            .map(|e| (e.event.source_chain_id, e.event.nonce))
            .collect();
        assert_eq!(keys, vec![(1, 3), (1, 20), (42161, 1), (42161, 5)]);
    }

    /// `update_pending_entry` 写入后，`load_all_pending_events` 能读回 submission。
    #[test]
    fn update_then_load_preserves_submission() {
        let tmp = tempdir().unwrap();
        let entry = PendingEntry {
            event: sample_event(91024, 1, 5),
            submission: Some(Submission {
                tx_hash: "0xdeadbeef".into(),
                sent_at_unix: 1_700_000_000,
                mined_block: Some(42_000),
            }),
        };
        update_pending_entry(tmp.path(), &entry).unwrap();
        let loaded = load_all_pending_events(tmp.path(), 91024).unwrap();
        assert_eq!(loaded, vec![entry]);
    }

    /// `update_pending_entry` 多次调用应该是覆盖语义，而不是 no-op。
    #[test]
    fn update_overwrites() {
        let tmp = tempdir().unwrap();
        let mut entry = PendingEntry {
            event: sample_event(91024, 1, 5),
            submission: Some(Submission {
                tx_hash: "0xv1".into(),
                sent_at_unix: 1_700_000_000,
                mined_block: None,
            }),
        };
        update_pending_entry(tmp.path(), &entry).unwrap();
        // 改动 submission，再次写入
        entry.submission.as_mut().unwrap().mined_block = Some(123);
        entry.submission.as_mut().unwrap().tx_hash = "0xv2".into();
        update_pending_entry(tmp.path(), &entry).unwrap();

        let loaded = load_all_pending_events(tmp.path(), 91024).unwrap();
        assert_eq!(loaded, vec![entry]);
    }

    /// 文件名解析：合法 / 非法 / 缺分隔符 / 非数字。
    #[test]
    fn parse_event_filename_handles_edges() {
        assert_eq!(parse_event_filename("1_42"), Some((1, 42)));
        assert_eq!(parse_event_filename("91024_0"), Some((91024, 0)));
        assert_eq!(parse_event_filename("nope"), None);
        assert_eq!(parse_event_filename("a_b"), None);
        assert_eq!(parse_event_filename("1_"), None);
        assert_eq!(parse_event_filename("_42"), None);
    }

    /// 目录里混入非法文件名 / 非 .json / 解析失败的 json，
    /// 应该 warn 跳过但不影响合法文件加载。
    #[test]
    fn load_skips_invalid_files() {
        let tmp = tempdir().unwrap();
        let ev = sample_event(91024, 1, 5);
        save_pending_event(tmp.path(), &ev).unwrap();

        let dir = target_dir(tmp.path(), 91024);
        std::fs::write(dir.join("garbage.txt"), "x").unwrap(); // 错后缀
        std::fs::write(dir.join("notvalid.json"), "x").unwrap(); // 文件名不规范
        std::fs::write(dir.join("1_99.json"), "{not json}").unwrap(); // 解析失败

        let loaded = load_all_pending_events(tmp.path(), 91024).unwrap();
        assert_eq!(loaded, vec![empty_entry(91024, 1, 5)]);
    }
}
