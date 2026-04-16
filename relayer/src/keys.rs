//! 密钥管理模块
//!
//! 负责 SVM (Solana) Keypair 和 EVM 私钥的加载与自动生成。
//! - 首次启动时自动生成密钥对，保存到 {data_dir}/keys/
//! - 同时输出 addresses.json 文件和 WARN 级别日志，提醒运维去桥合约添加白名单
//! - 后续启动直接从文件加载

use std::path::Path;

use anyhow::{Context, Result};
use ethers::signers::{LocalWallet, Signer};
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer as SolSigner;
use tracing::warn;

/// 已加载的 relayer 密钥材料
pub struct Keys {
    /// SVM (Solana) 密钥对 —— 用于在 1024 链上签名 confirm_event 交易
    pub svm_keypair: Keypair,
    /// EVM 钱包 —— 用于在 EVM peer 链上签名 confirmEvent 交易
    pub evm_wallet: LocalWallet,
}

/// 地址信息，序列化后写入 addresses.json 供运维查看
#[derive(serde::Serialize)]
struct Addresses {
    svm_pubkey: String,
    evm_address: String,
}

impl Keys {
    /// 从 `keys_dir` 加载密钥（不存在则自动生成），并写入 `addresses.json`。
    ///
    /// 自动生成时会用 WARN 级别打印公钥/地址，提醒运维去合约上添加 relayer 白名单。
    pub fn load_or_generate(keys_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(keys_dir)?;

        let svm_path = keys_dir.join("svm_keypair.json");
        let evm_path = keys_dir.join("evm_private_key.txt");

        // 加载或生成 SVM Keypair（Solana 格式：JSON 数组 of 64 bytes）
        let svm_keypair = load_or_generate_svm(&svm_path)?;
        // 加载或生成 EVM 私钥（纯 hex 字符串，无 0x 前缀也可以）
        let evm_wallet = load_or_generate_evm(&evm_path)?;

        let svm_pubkey = svm_keypair.pubkey().to_string();
        let evm_address = format!("{:?}", evm_wallet.address());

        // 将两个地址写入 addresses.json，方便运维查看和复制
        let addresses = Addresses {
            svm_pubkey: svm_pubkey.clone(),
            evm_address: evm_address.clone(),
        };
        let addr_path = keys_dir.join("addresses.json");
        let json = serde_json::to_string_pretty(&addresses)?;
        std::fs::write(&addr_path, json)
            .with_context(|| format!("写入 {} 失败", addr_path.display()))?;

        // WARN 级别打印，确保运维在 docker logs 中能看到
        warn!(svm_pubkey = %svm_pubkey, "SVM 公钥（需要添加到桥合约 relayer 白名单）");
        warn!(evm_address = %evm_address, "EVM 地址（需要添加到桥合约 relayer 白名单）");

        Ok(Keys {
            svm_keypair,
            evm_wallet,
        })
    }
}

/// 加载或生成 SVM Keypair。
/// 文件格式：JSON 数组，内容为 64 字节的 keypair（与 solana-keygen 兼容）。
fn load_or_generate_svm(path: &Path) -> Result<Keypair> {
    if path.exists() {
        // 从文件读取已有的 keypair
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("读取 {} 失败", path.display()))?;
        let bytes: Vec<u8> = serde_json::from_str(&data)
            .with_context(|| format!("{} 中的 keypair JSON 格式无效", path.display()))?;
        let kp = Keypair::try_from(bytes.as_slice())
            .with_context(|| "Solana keypair bytes 无效")?;
        Ok(kp)
    } else {
        // 首次启动：随机生成新的 keypair 并保存
        let kp = Keypair::new();
        let bytes = kp.to_bytes().to_vec();
        let json = serde_json::to_string(&bytes)?;
        std::fs::write(path, &json)
            .with_context(|| format!("写入 {} 失败", path.display()))?;
        warn!("已自动生成 SVM keypair: {}", path.display());
        Ok(kp)
    }
}

/// 加载或生成 EVM 私钥。
/// 文件格式：纯 hex 字符串（支持 0x 前缀）。
fn load_or_generate_evm(path: &Path) -> Result<LocalWallet> {
    if path.exists() {
        // 从文件读取已有的私钥
        let hex_str = std::fs::read_to_string(path)
            .with_context(|| format!("读取 {} 失败", path.display()))?;
        let hex_str = hex_str.trim().trim_start_matches("0x");
        let wallet: LocalWallet = hex_str
            .parse()
            .with_context(|| "EVM 私钥 hex 格式无效")?;
        Ok(wallet)
    } else {
        // 首次启动：随机生成新的 EVM 私钥并保存
        let mut rng = rand::thread_rng();
        let wallet = LocalWallet::new(&mut rng);
        let key_bytes = wallet.signer().to_bytes();
        let hex_str = hex::encode(key_bytes);
        std::fs::write(path, &hex_str)
            .with_context(|| format!("写入 {} 失败", path.display()))?;
        warn!("已自动生成 EVM 私钥: {}", path.display());
        Ok(wallet)
    }
}
