//! 日志初始化模块
//!
//! 配置双输出 JSON 格式日志：
//! 1. stderr —— 供 `docker logs` 查看容器日志
//! 2. 文件（{logs_dir}/relayer.log）—— 按天轮转，最多保留两周
//!
//! 日志级别通过 `RUST_LOG` 环境变量控制，默认为 `info`。

use std::path::Path;

use anyhow::{Context, Result};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// 文件日志保留天数（按天轮转，超过该数量的旧文件自动删除）。
const LOG_RETENTION_DAYS: usize = 14;

/// 初始化日志系统。
///
/// - `logs_dir`：日志文件的存储目录；**由调用方（`Config::ensure_dirs`）负责创建**，
///   本函数不再重复 `create_dir_all`，避免职责分散
/// - 两个输出层（stderr 和文件）各自独立的 EnvFilter，都读 RUST_LOG 或默认 info
/// - 文件按天轮转 (`relayer.log.YYYY-MM-DD`)，自动清理 >14 天的旧文件
pub fn init(logs_dir: &Path) -> Result<()> {
    // stderr 层的日志级别过滤
    let env_filter_stderr = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    // 文件层的日志级别过滤
    let env_filter_file = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // 按天轮转 + 保留 14 个文件：超过的旧文件由 appender 在轮转时自动删除，
    // 既避免磁盘被无限增长的日志撑爆，又留出足够时间排查两周内的事故。
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("relayer.log")
        .max_log_files(LOG_RETENTION_DAYS)
        .build(logs_dir)
        .context("初始化按天轮转的日志 appender")?;

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
