use crate::error::Result;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// 初始化日志系统（仅控制台输出）
pub fn init_logger(level: &str, format: &str) -> Result<()> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    if format == "json" {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().pretty())
            .init();
    }

    Ok(())
}

/// Guard that must be held alive for the file appender to keep writing.
pub struct LogFileGuard {
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

/// 初始化日志系统，同时输出到控制台和文件。
/// 返回的 guard 必须在 main 中保持存活，否则日志文件会提前关闭。
pub fn init_logger_with_file(
    level: &str,
    format: &str,
    log_file: Option<&str>,
) -> Result<Option<LogFileGuard>> {
    let log_file = match log_file {
        Some(p) => p,
        None => {
            init_logger(level, format)?;
            return Ok(None);
        }
    };

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    let path = std::path::Path::new(log_file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| {
            crate::error::RelayerError::Config(format!(
                "Failed to open log file '{}': {}",
                path.display(),
                e
            ))
        })?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_writer(non_blocking);

    let is_json = format == "json";

    tracing_subscriber::registry()
        .with(env_filter)
        .with(is_json.then(|| fmt::layer().json()))
        .with((!is_json).then(fmt::layer))
        .with(file_layer)
        .init();

    Ok(Some(LogFileGuard { _guard: guard }))
}

/// 日志宏的简化封装
#[macro_export]
macro_rules! log_event {
    (received, nonce = $nonce:expr, amount = $amount:expr, receiver = $receiver:expr) => {
        tracing::info!(
            nonce = $nonce,
            amount = $amount,
            receiver = $receiver,
            "Received stake event"
        );
    };
    (signed, nonce = $nonce:expr) => {
        tracing::info!(nonce = $nonce, "Signature generated");
    };
    (submitted, nonce = $nonce:expr, tx_hash = $tx_hash:expr) => {
        tracing::info!(
            nonce = $nonce,
            tx_hash = $tx_hash,
            "Signature submitted"
        );
    };
    (confirmed, nonce = $nonce:expr, tx_hash = $tx_hash:expr) => {
        tracing::info!(
            nonce = $nonce,
            tx_hash = $tx_hash,
            "Transaction confirmed"
        );
    };
    (failed, nonce = $nonce:expr, error = $error:expr) => {
        tracing::error!(
            nonce = $nonce,
            error = %$error,
            "Transaction failed"
        );
    };
}

