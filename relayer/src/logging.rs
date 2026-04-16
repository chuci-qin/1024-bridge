//! 日志初始化模块
//!
//! 配置双输出 JSON 格式日志：
//! 1. stderr —— 供 `docker logs` 查看容器日志
//! 2. 文件（{logs_dir}/relayer.log）—— 按天轮转，持久化保存
//!
//! 日志级别通过 `RUST_LOG` 环境变量控制，默认为 `info`。

use std::path::Path;

use anyhow::Result;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// 初始化日志系统。
///
/// - `logs_dir`：日志文件的存储目录，不存在会自动创建
/// - 两个输出层（stderr 和文件）各自独立的 EnvFilter，都读 RUST_LOG 或默认 info
pub fn init(logs_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(logs_dir)?;

    // stderr 层的日志级别过滤
    let env_filter_stderr = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    // 文件层的日志级别过滤
    let env_filter_file = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // 每天轮转的文件 appender：logs_dir/relayer.log → relayer.log.2024-01-01 ...
    let file_appender = RollingFileAppender::new(Rotation::DAILY, logs_dir, "relayer.log");

    // stderr 层：JSON 格式
    let stderr_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(std::io::stderr)
        .with_filter(env_filter_stderr);

    // 文件层：JSON 格式
    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(file_appender)
        .with_filter(env_filter_file);

    // 注册两个层到全局 subscriber
    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();

    Ok(())
}
