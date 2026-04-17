//! 链注册表模块
//!
//! 维护一个硬编码的链信息表，包含所有支持的 EVM 和 SVM 链：
//! - chain_id：链的唯一标识（EVM 用官方 chain_id，SVM 用自定义约定）
//! - env_name：RPC 环境变量的后缀名（如 RPC_ETHEREUM_MAINNET）
//! - default_rpc：免费/公共 RPC 的默认 URL
//! - kind：链类型（EVM 或 SVM）
//!
//! RPC 选择优先级：环境变量 `RPC_{env_name}` > 默认 RPC URL

use std::env;

use crate::types::ChainKind;

/// 一条链的静态元数据
#[derive(Clone, Debug)]
pub struct ChainInfo {
    /// 链唯一标识
    pub chain_id: u64,
    /// 用于环境变量覆盖 RPC 的键名后缀（如 "ETHEREUM_MAINNET" → 环境变量 RPC_ETHEREUM_MAINNET）
    pub env_name: &'static str,
    /// 公共 RPC 的默认 URL
    pub default_rpc: &'static str,
    /// 虚拟机类型
    pub kind: ChainKind,
    /// 跨链桥确认数（仅 EVM 用，SVM 链填 0）。
    ///
    /// **统一含义**：reorg 深度容忍上限。
    /// - **Poller 侧**：safe_head = latest_block - confirmations，
    ///   只读 safe_head 之前的事件（防止源链 reorg 让 relayer 处理"假事件"）。
    /// - **Submitter 侧**：发出 confirm tx 后等 N confirmations 才视为成功，
    ///   防止 target 链 reorg 把已删除的事件文件留下空账。
    ///
    /// 取值依据：覆盖该链 99.9% 历史 reorg 深度。参考主流跨链桥
    /// （Wormhole / LayerZero / Stargate）的稳态档：
    /// - Ethereum 12（~2.4min）—— PoW 时代经典 12 块
    /// - Sepolia 6（~1.2min）—— 测试网容忍
    /// - Arbitrum 20（~20s）—— sequencer 模式 + 抗短期故障
    /// - Base 10（~20s）—— sequencer 模式
    ///
    /// 可通过环境变量 `EVM_CONFIRMATIONS_<chain_id>` 按链覆盖。
    pub confirmations: u64,
    /// 一笔 EVM confirm tx 广播后多少秒内若 receipt 仍未出现，就视为"丢失"
    /// 触发 stale 流程（check mempool → replacement 或 self-transfer）。
    ///
    /// 按链分档而不用全局常量的理由：
    /// - L1（ETH/Sepolia）12s/block，mempool 滞留几十秒很正常 → 600s 相对宽容
    /// - Arbitrum/Optimism sequencer 模式 ~250ms/block，几十秒没 mined 就极可能丢了 → 60s
    /// - Base sequencer 模式 ~2s/block，120s 覆盖常见延迟 + 留 2 倍余量
    /// - SVM 链不走此路径（由 `STALE_PENDING_SVM_TX_SECS` 主常量管），这里填 0
    ///
    /// 可通过环境变量 `EVM_STALE_PENDING_TX_SECS_<chain_id>` 按链覆盖。
    pub stale_pending_tx_secs: u64,
}

/// 所有支持的链列表（硬编码）。`confirmations` 字段语义见 `ChainInfo` 文档。
const CHAINS: &[ChainInfo] = &[
    // ── Ethereum ──
    ChainInfo {
        chain_id: 1,
        env_name: "ETHEREUM_MAINNET",
        default_rpc: "https://ethereum-rpc.publicnode.com",
        kind: ChainKind::Evm,
        confirmations: 12,
        stale_pending_tx_secs: 600,
    },
    ChainInfo {
        chain_id: 11155111,
        env_name: "ETHEREUM_SEPOLIA",
        default_rpc: "https://ethereum-sepolia-rpc.publicnode.com",
        kind: ChainKind::Evm,
        confirmations: 6,
        stale_pending_tx_secs: 600,
    },
    // ── Arbitrum ──
    ChainInfo {
        chain_id: 42161,
        env_name: "ARBITRUM_MAINNET",
        default_rpc: "https://arbitrum-one-rpc.publicnode.com",
        kind: ChainKind::Evm,
        confirmations: 20,
        stale_pending_tx_secs: 60,
    },
    ChainInfo {
        chain_id: 421614,
        env_name: "ARBITRUM_SEPOLIA",
        default_rpc: "https://sepolia-rollup.arbitrum.io/rpc",
        kind: ChainKind::Evm,
        confirmations: 20,
        stale_pending_tx_secs: 60,
    },
    // ── Base ──
    ChainInfo {
        chain_id: 8453,
        env_name: "BASE_MAINNET",
        default_rpc: "https://mainnet.base.org",
        kind: ChainKind::Evm,
        confirmations: 10,
        stale_pending_tx_secs: 120,
    },
    ChainInfo {
        chain_id: 84532,
        env_name: "BASE_SEPOLIA",
        default_rpc: "https://sepolia.base.org",
        kind: ChainKind::Evm,
        confirmations: 10,
        stale_pending_tx_secs: 120,
    },
    // ── Solana ──
    ChainInfo {
        chain_id: 101,
        env_name: "SOLANA_MAINNET",
        default_rpc: "https://api.mainnet-beta.solana.com",
        kind: ChainKind::Svm,
        confirmations: 0,
        stale_pending_tx_secs: 0,
    },
    ChainInfo {
        chain_id: 103,
        env_name: "SOLANA_DEVNET",
        default_rpc: "https://api.devnet.solana.com",
        kind: ChainKind::Svm,
        confirmations: 0,
        stale_pending_tx_secs: 0,
    },
    // ── 1024 Chain ──
    ChainInfo {
        chain_id: 91024,
        env_name: "1024_MAINNET",
        default_rpc: "https://rpc.1024chain.com",
        kind: ChainKind::Svm,
        confirmations: 0,
        stale_pending_tx_secs: 0,
    },
    ChainInfo {
        chain_id: 91025,
        env_name: "1024_TESTNET",
        // L9 修复：原先 URL 末尾多了 "/rpc/"，与其它 1024 RPC 不一致，
        // 容易让管理员误以为需要带上后缀。统一为 publicnode 风格。
        default_rpc: "https://rpc-testnet.1024chain.com",
        kind: ChainKind::Svm,
        confirmations: 0,
        stale_pending_tx_secs: 0,
    },
    ChainInfo {
        chain_id: 91026,
        env_name: "1024_STABLENET",
        default_rpc: "https://rpc-testnet-stable.1024chain.com",
        kind: ChainKind::Svm,
        confirmations: 0,
        stale_pending_tx_secs: 0,
    },
];

/// 根据 chain_id 查找链信息。找不到返回 None。
pub fn get_chain_info(chain_id: u64) -> Option<&'static ChainInfo> {
    CHAINS.iter().find(|c| c.chain_id == chain_id)
}

/// 拿一条 EVM 链的 confirmations 配置：
/// 1. 优先环境变量 `EVM_CONFIRMATIONS_<chain_id>`（运维按业务调档，比如紧急加保守）
/// 2. 否则用 `chain_registry` 硬编码的工业标准值
/// 3. 未注册 chain_id 且无环境变量 → None，由调用方决定（建议 bail）
///
/// SVM 链此值返回 0（SVM 不走 confirmations 模型，用 commitment）。
pub fn confirmations(chain_id: u64) -> Option<u64> {
    let env_key = format!("EVM_CONFIRMATIONS_{chain_id}");
    if let Ok(raw) = env::var(&env_key) {
        match raw.trim().parse::<u64>() {
            Ok(n) => return Some(n),
            Err(_) => tracing::warn!(
                env_key = %env_key,
                value = %raw,
                "{env_key} 不是合法 u64，忽略环境变量并回退到默认值"
            ),
        }
    }
    get_chain_info(chain_id).map(|c| c.confirmations)
}

/// 拿一条 EVM 链的"广播后多久视为 stale"阈值（秒）：
/// 1. 优先环境变量 `EVM_STALE_PENDING_TX_SECS_<chain_id>`
/// 2. 否则用 `chain_registry` 硬编码的工业标准值
/// 3. 未注册 chain_id 且无环境变量 → None，由调用方决定默认
///
/// SVM 链返回 0（不走此路径）。
pub fn stale_pending_tx_secs(chain_id: u64) -> Option<u64> {
    let env_key = format!("EVM_STALE_PENDING_TX_SECS_{chain_id}");
    if let Ok(raw) = env::var(&env_key) {
        match raw.trim().parse::<u64>() {
            Ok(n) => return Some(n),
            Err(_) => tracing::warn!(
                env_key = %env_key,
                value = %raw,
                "{env_key} 不是合法 u64，忽略环境变量并回退到默认值"
            ),
        }
    }
    get_chain_info(chain_id).map(|c| c.stale_pending_tx_secs)
}

/// 解析最终使用的 RPC URL：优先检查环境变量 `RPC_{env_name}`，否则用默认值。
///
/// 例如对 Ethereum Mainnet（env_name = "ETHEREUM_MAINNET"），
/// 会先检查 `RPC_ETHEREUM_MAINNET` 环境变量。
pub fn resolve_rpc(info: &ChainInfo) -> String {
    let env_key = format!("RPC_{}", info.env_name);
    env::var(&env_key).unwrap_or_else(|_| info.default_rpc.to_string())
}

/// 将 `BRIDGE_1024_NETWORK` 的值映射为 1024 链的 chain_id。
///
/// 接受的值（不区分大小写）：
/// - `"mainnet"`              → 91024 （对应 env_name `1024_MAINNET`）
/// - `"testnet"`              → 91025 （对应 env_name `1024_TESTNET`）
/// - `"stablenet"` / `"stable"` → 91026 （对应 env_name `1024_STABLENET`）
///
/// L9：保留 `"stable"` 作为 `"stablenet"` 的别名以兼容旧配置，但 env_name
/// 与日志中规范名称统一为 `STABLENET`。
pub fn network_to_chain_id(network: &str) -> Option<u64> {
    match network.to_lowercase().as_str() {
        "mainnet" => Some(91024),
        "testnet" => Some(91025),
        "stablenet" | "stable" => Some(91026),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// chain_id 必须全局唯一 —— 防止有人加新链时复制粘贴留了重复。
    #[test]
    fn all_chain_ids_are_unique() {
        let mut ids: Vec<u64> = CHAINS.iter().map(|c| c.chain_id).collect();
        ids.sort_unstable();
        let original_len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), original_len, "存在重复的 chain_id");
    }

    /// env_name 必须全局唯一 —— 防止两个链共享同一个 RPC_ 环境变量。
    #[test]
    fn all_env_names_are_unique() {
        let mut names: Vec<&str> = CHAINS.iter().map(|c| c.env_name).collect();
        names.sort_unstable();
        let original_len = names.len();
        names.dedup();
        assert_eq!(names.len(), original_len, "存在重复的 env_name");
    }

    /// L9 回归：1024 三网 RPC URL 不应有"/rpc/"后缀（以前 testnet 误录入过）。
    #[test]
    fn chain_1024_rpc_urls_have_no_rpc_suffix() {
        for c in CHAINS.iter().filter(|c| c.env_name.starts_with("1024_")) {
            assert!(
                !c.default_rpc.ends_with("/rpc/"),
                "{}: default_rpc 末尾不应是 /rpc/，得到 {}",
                c.env_name,
                c.default_rpc
            );
        }
    }

    #[test]
    fn network_to_chain_id_accepts_known_inputs() {
        assert_eq!(network_to_chain_id("mainnet"), Some(91024));
        assert_eq!(network_to_chain_id("MAINNET"), Some(91024)); // case insensitive
        assert_eq!(network_to_chain_id("testnet"), Some(91025));
        assert_eq!(network_to_chain_id("stablenet"), Some(91026));
        assert_eq!(network_to_chain_id("stable"), Some(91026)); // 兼容别名
        assert_eq!(network_to_chain_id("unknown"), None);
        assert_eq!(network_to_chain_id(""), None);
    }

    /// EVM 链必须配置非零 confirmations，SVM 链必须为 0（不走此模型）。
    #[test]
    fn confirmations_set_per_kind() {
        for c in CHAINS {
            match c.kind {
                ChainKind::Evm => assert!(
                    c.confirmations > 0,
                    "{}: EVM 链必须配置非零 confirmations",
                    c.env_name
                ),
                ChainKind::Svm => assert_eq!(
                    c.confirmations, 0,
                    "{}: SVM 链应配置 confirmations=0",
                    c.env_name
                ),
            }
        }
    }

    /// EVM 链必须配置非零 stale_pending_tx_secs（否则上线第一秒就进 stale 分支），
    /// SVM 链必须为 0（SVM 走独立的 STALE_PENDING_SVM_TX_SECS 常量，不用此字段）。
    #[test]
    fn stale_pending_tx_secs_set_per_kind() {
        for c in CHAINS {
            match c.kind {
                ChainKind::Evm => assert!(
                    c.stale_pending_tx_secs > 0,
                    "{}: EVM 链必须配置非零 stale_pending_tx_secs",
                    c.env_name
                ),
                ChainKind::Svm => assert_eq!(
                    c.stale_pending_tx_secs, 0,
                    "{}: SVM 链应配置 stale_pending_tx_secs=0（不走此路径）",
                    c.env_name
                ),
            }
        }
    }

    /// L2 分档合理性：sequencer 模式的 L2（Arbitrum/Base）不应超过 L1 默认档，
    /// 不然 L2 的"快 finality"优势就被 stale 阈值拖平了。
    #[test]
    fn stale_pending_tx_secs_l2_faster_than_l1() {
        let eth = stale_pending_tx_secs(1).unwrap();
        assert!(stale_pending_tx_secs(42161).unwrap() < eth, "Arbitrum 不应 >= ETH");
        assert!(stale_pending_tx_secs(8453).unwrap() < eth, "Base 不应 >= ETH");
    }

    /// env var 覆盖 stale_pending_tx_secs 的路径，和 confirmations_default_and_env_override
    /// 一样用未注册 chain_id 避免污染其它测试。
    #[test]
    fn stale_pending_tx_secs_env_override() {
        // 默认值快照
        assert_eq!(stale_pending_tx_secs(1), Some(600)); // ETH
        assert_eq!(stale_pending_tx_secs(42161), Some(60)); // Arbitrum
        assert_eq!(stale_pending_tx_secs(91024), Some(0)); // SVM 链
        assert_eq!(stale_pending_tx_secs(88888), None); // 未注册

        std::env::set_var("EVM_STALE_PENDING_TX_SECS_88888", "30");
        assert_eq!(stale_pending_tx_secs(88888), Some(30));
        std::env::remove_var("EVM_STALE_PENDING_TX_SECS_88888");

        std::env::set_var("EVM_STALE_PENDING_TX_SECS_88888", "garbage");
        assert_eq!(stale_pending_tx_secs(88888), None);
        std::env::remove_var("EVM_STALE_PENDING_TX_SECS_88888");
    }

    /// 默认值 + env var 覆盖 + 非法 env var 兜底，三种场景合在一个测试里串行验证。
    ///
    /// 必须串行的原因：`std::env` 是进程全局可变状态，cargo 默认多线程并行跑测试，
    /// 拆成多个测试函数同时 set/remove 同一个 EVM_CONFIRMATIONS_* 会互相污染。
    /// 这里用一个不在 chain_registry 中注册的 chain_id (99999) 测试 env var 路径，
    /// 与已注册链测试默认值的部分完全互斥，零交叉污染。
    #[test]
    fn confirmations_default_and_env_override() {
        // ── 1. 默认值快照（不碰 env） ──
        assert_eq!(confirmations(1), Some(12)); // ETH
        assert_eq!(confirmations(42161), Some(20)); // Arbitrum
        assert_eq!(confirmations(91024), Some(0)); // SVM 链 = 0
        assert_eq!(confirmations(99999), None); // 未注册链 = None

        // ── 2. env var 覆盖：用一个未注册的 chain_id，不影响其它测试 ──
        std::env::set_var("EVM_CONFIRMATIONS_99999", "42");
        assert_eq!(confirmations(99999), Some(42));
        std::env::remove_var("EVM_CONFIRMATIONS_99999");
        assert_eq!(confirmations(99999), None); // 还原

        // ── 3. 非法值应被忽略，未注册链回退 None ──
        std::env::set_var("EVM_CONFIRMATIONS_99999", "not-a-number");
        assert_eq!(confirmations(99999), None);
        std::env::remove_var("EVM_CONFIRMATIONS_99999");
    }
}
