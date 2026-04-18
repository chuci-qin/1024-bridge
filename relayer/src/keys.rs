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

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// 密钥/私钥文件的目标 mode：仅文件所有者可读写。
#[cfg(unix)]
const SECRET_FILE_MODE: u32 = 0o600;

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

/// 以 0600 权限原子写入私密文件（先写 .tmp 再 rename）。
///
/// 行为：
/// - Unix：用 `OpenOptionsExt::mode(0o600)` 在 open 时就受 umask 影响之外强制权限位；
/// - 非 Unix：fallback 到普通写入（生产部署都是 Linux/Docker，覆盖足够）。
fn write_secret(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");

    #[cfg(unix)]
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(SECRET_FILE_MODE)
            .open(&tmp)
            .with_context(|| format!("打开 {} 失败", tmp.display()))?;
        f.write_all(contents)
            .with_context(|| format!("写入 {} 失败", tmp.display()))?;
        f.sync_all().ok();
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, contents)
            .with_context(|| format!("写入 {} 失败", tmp.display()))?;
    }

    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {} 失败", tmp.display(), path.display()))?;
    Ok(())
}

/// 加载已有私密文件时，如果权限太宽（others/group 可读写），收紧到 0600 并 warn。
///
/// 这种情况通常是历史遗留：之前版本用默认 umask（通常 0644）写过文件，
/// 升级后我们主动把它们 chmod 回 0600，避免任何"私钥曾以 0644 写盘"的窗口。
fn ensure_secret_perms(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let perms = std::fs::metadata(path)
            .with_context(|| format!("读取 {} metadata 失败", path.display()))?
            .permissions();
        let mode = perms.mode() & 0o777;
        if mode & 0o077 != 0 {
            warn!(
                path = %path.display(),
                current = format!("{:o}", mode),
                "私密文件权限过宽，已收紧到 0600"
            );
            let mut new_perms = perms;
            new_perms.set_mode(SECRET_FILE_MODE);
            std::fs::set_permissions(path, new_perms)
                .with_context(|| format!("chmod {} 失败", path.display()))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path; // 非 Unix 不强制
    }
    Ok(())
}

impl Keys {
    /// 从 `keys_dir` 加载密钥（不存在则自动生成），并写入 `addresses.json`。
    ///
    /// 自动生成时会用 WARN 级别打印公钥/地址，提醒运维去合约上添加 relayer 白名单。
    /// 私钥文件以 0600 权限写入；加载已有文件时若权限过宽会自动收紧并 warn。
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

        // 只打印两个地址 —— 是否已注册到各链桥合约白名单，由 main.rs 的
        // `verify_relayer_whitelist` 在 endpoints 构造完之后再去链上查，
        // 那里才能给出"哪条链没注册"的真实结论；这里盲目 warn 反而会
        // 在已注册场景下持续噪音。
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
        // 加载前先收紧权限
        ensure_secret_perms(path)?;

        let data = std::fs::read_to_string(path)
            .with_context(|| format!("读取 {} 失败", path.display()))?;
        let bytes: Vec<u8> = serde_json::from_str(&data)
            .with_context(|| format!("{} 中的 keypair JSON 格式无效", path.display()))?;
        let kp = Keypair::try_from(bytes.as_slice())
            .with_context(|| "Solana keypair bytes 无效")?;
        Ok(kp)
    } else {
        // 首次启动：随机生成新的 keypair 并以 0600 权限写盘
        let kp = Keypair::new();
        let bytes = kp.to_bytes().to_vec();
        let json = serde_json::to_string(&bytes)?;
        write_secret(path, json.as_bytes())?;
        warn!("已自动生成 SVM keypair: {} (mode 0600)", path.display());
        Ok(kp)
    }
}

/// 加载或生成 EVM 私钥。
/// 文件格式：纯 hex 字符串（支持 0x 前缀）。
fn load_or_generate_evm(path: &Path) -> Result<LocalWallet> {
    if path.exists() {
        ensure_secret_perms(path)?;

        let hex_str = std::fs::read_to_string(path)
            .with_context(|| format!("读取 {} 失败", path.display()))?;
        let hex_str = hex_str.trim().trim_start_matches("0x");
        let wallet: LocalWallet = hex_str
            .parse()
            .with_context(|| "EVM 私钥 hex 格式无效")?;
        Ok(wallet)
    } else {
        // 首次启动：随机生成新的 EVM 私钥并以 0600 权限写盘
        let mut rng = rand::thread_rng();
        let wallet = LocalWallet::new(&mut rng);
        let key_bytes = wallet.signer().to_bytes();
        let hex_str = hex::encode(key_bytes);
        write_secret(path, hex_str.as_bytes())?;
        warn!("已自动生成 EVM 私钥: {} (mode 0600)", path.display());
        Ok(wallet)
    }
}
