//! SVM 签名磁盘队列
//!
//! 负责 `sigs/{chain_id}/` 与 `sigs_dead/{chain_id}/` 两个目录的纯文件名操作。
//!
//! 磁盘形态：空文件，文件名 = base58 signature，无扩展名。
//! 所有 attempt 元数据（attempt_count / last_attempt_at）由 extractor 持有的
//! in-memory `HashMap<Signature, AttemptState>` 管理，进程重启即丢失（全部 fresh）。
//!
//! 崩溃一致性：空文件天然原子 —— `File::create_new` = O_CREAT|O_EXCL 写 0 字节、
//! `fs::rename` 单步搬到 DLQ、`fs::remove_file` 单步删除。没有 `.tmp` 中间路径。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use solana_sdk::signature::Signature;

/// 进程内可变的 attempt 状态，仅 extractor 持有，进程崩溃即丢失（重启全部 fresh）。
#[derive(Clone, Debug, Default)]
pub struct AttemptState {
    pub last_attempt_at: u64,
    pub attempt_count: u32,
}

/// 在 `active_dir/{base58_sig}` 创建 0 字节空文件。
///
/// 已存在视为 idempotent 成功（enumerator 重启后同一 sig 会被重新枚举）。
pub fn save_new_sig(active_dir: &Path, sig: &Signature) -> Result<()> {
    let path = active_dir.join(sig.to_string());
    match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| format!("创建 sig 文件失败: {}", path.display())),
    }
}

/// 列出 active 目录下所有 sig 文件名（解析为 `Signature`）。
///
/// 跳过子目录和无法解析为 base58 Signature 的文件名（防御性）。
pub fn list_active_sigs(active_dir: &Path) -> Result<Vec<Signature>> {
    if !active_dir.exists() {
        return Ok(Vec::new());
    }
    let read_dir = fs::read_dir(active_dir)
        .with_context(|| format!("读取 sig 目录失败: {}", active_dir.display()))?;

    let mut sigs = Vec::new();
    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("枚举 sig 目录条目失败: {e}");
                continue;
            }
        };
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else { continue };
        match name_str.parse::<Signature>() {
            Ok(sig) => sigs.push(sig),
            Err(_) => {
                tracing::warn!("跳过非 base58 Signature 文件名: {name_str}");
            }
        }
    }
    Ok(sigs)
}

/// 成功路径：单步 `fs::remove_file`。文件不存在视为幂等成功。
pub fn delete_sig(active_dir: &Path, sig: &Signature) -> Result<()> {
    let path = active_dir.join(sig.to_string());
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("删除 sig 文件失败: {}", path.display())),
    }
}

/// 超阈值路径：单步 `fs::rename` 从 active 搬到 dead。
pub fn move_to_dead_letter(active_dir: &Path, dead_dir: &Path, sig: &Signature) -> Result<()> {
    let name = sig.to_string();
    let src = active_dir.join(&name);
    let dst = dead_dir.join(&name);
    fs::rename(&src, &dst).with_context(|| {
        format!(
            "移动 sig 到 DLQ 失败: {} -> {}",
            src.display(),
            dst.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer;
    use tempfile::tempdir;

    fn random_sig() -> Signature {
        let kp = Keypair::new();
        kp.sign_message(b"test")
    }

    #[test]
    fn save_new_sig_is_idempotent() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let sig = random_sig();

        save_new_sig(dir, &sig).unwrap();
        save_new_sig(dir, &sig).unwrap(); // second call must not error

        let path = dir.join(sig.to_string());
        assert!(path.exists());
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    }

    #[test]
    fn list_active_sigs_skips_subdirs_and_bad_names() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let sig = random_sig();

        save_new_sig(dir, &sig).unwrap();
        fs::create_dir(dir.join("subdir")).unwrap();
        fs::write(dir.join("not-a-sig.txt"), b"").unwrap();

        let listed = list_active_sigs(dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], sig);
    }

    #[test]
    fn delete_sig_is_idempotent() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path();
        let sig = random_sig();

        delete_sig(dir, &sig).unwrap(); // delete non-existent → Ok

        save_new_sig(dir, &sig).unwrap();
        delete_sig(dir, &sig).unwrap();
        assert!(!dir.join(sig.to_string()).exists());
    }

    #[test]
    fn move_to_dead_letter_atomic() {
        let tmp = tempdir().unwrap();
        let active = tmp.path().join("active");
        let dead = tmp.path().join("dead");
        fs::create_dir(&active).unwrap();
        fs::create_dir(&dead).unwrap();

        let sig = random_sig();
        save_new_sig(&active, &sig).unwrap();
        assert!(active.join(sig.to_string()).exists());

        move_to_dead_letter(&active, &dead, &sig).unwrap();
        assert!(!active.join(sig.to_string()).exists());
        assert!(dead.join(sig.to_string()).exists());
    }
}
