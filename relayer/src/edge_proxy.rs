//! Edge Proxy URL 重写 — 把「中国大陆访问慢/被墙」的链 RPC/WS 改走自建 HK 代理。
//!
//! 默认关闭: 未设 `EDGE_PROXY_BASE` 时 [`resolve_proxy_url`] 原样返回, 现有直连零改动。
//! 仅命中 [`EDGE_PROXY_MAP`] 的域名会被重写, 其余一律原样直连。
//! 前缀须与 HK 代理 nginx 配置一致: `edge-proxy/nginx/conf.d/10-edge-proxy.conf`。
//!
//! 纯 std 实现, 无第三方依赖。

/// 域名 → 代理前缀 (仅含大陆访问慢/被墙的第三方; 1024 自有域名不在内)。
const EDGE_PROXY_MAP: &[(&str, &str)] = &[
    // 区块链 RPC
    ("api.mainnet-beta.solana.com", "/sol-mainnet"),
    ("api.devnet.solana.com", "/sol-devnet"),
    ("ethereum-rpc.publicnode.com", "/eth"),
    ("ethereum-sepolia-rpc.publicnode.com", "/eth-sepolia"),
    ("arbitrum-one-rpc.publicnode.com", "/arb"),
    ("sepolia-rollup.arbitrum.io", "/arb-sepolia"),
    ("mainnet.base.org", "/base"),
    ("sepolia.base.org", "/base-sepolia"),
];

/// 读取 `EDGE_PROXY_BASE` (去掉末尾斜杠)。空字符串 = 代理关闭。
fn edge_proxy_base() -> String {
    std::env::var("EDGE_PROXY_BASE")
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

/// 代理是否启用 (设置了非空 `EDGE_PROXY_BASE`)。
pub fn is_edge_proxy_enabled() -> bool {
    !edge_proxy_base().is_empty()
}

/// 把链 RPC/WS URL 重写为走 HK 代理; 未启用或域名不在清单内则原样返回。
/// ws/wss 协议会自动把代理 base 的 http→ws、https→wss。
pub fn resolve_proxy_url(original_url: &str) -> String {
    let base = edge_proxy_base();
    if base.is_empty() {
        return original_url.to_string();
    }

    let (scheme, rest) = match original_url.split_once("://") {
        Some(v) => v,
        None => return original_url.to_string(),
    };
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{}", p)),
        None => (rest, String::new()),
    };
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);

    let prefix = match EDGE_PROXY_MAP.iter().find(|(h, _)| *h == host) {
        Some((_, p)) => *p,
        None => return original_url.to_string(),
    };

    let mut base = base;
    if scheme == "ws" || scheme == "wss" {
        if let Some(stripped) = base.strip_prefix("https:") {
            base = format!("wss:{}", stripped);
        } else if let Some(stripped) = base.strip_prefix("http:") {
            base = format!("ws:{}", stripped);
        }
    }
    format!("{}{}{}", base, prefix, path)
}
