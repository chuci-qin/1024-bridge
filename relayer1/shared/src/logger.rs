use crate::error::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// 初始化日志系统
pub fn init_logger(level: &str, format: &str) -> Result<()> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    if format == "json" {
        // JSON 格式 (生产环境)
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        // Pretty 格式 (开发环境)
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().pretty())
            .init();
    }

    Ok(())
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

