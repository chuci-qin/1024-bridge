use std::path::Path;

use anyhow::Result;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Initialize dual-output JSON logging:
/// - stderr: JSON format (for `docker logs`)
/// - `logs_dir/relayer.log`: JSON format, daily rotation
pub fn init(logs_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(logs_dir)?;

    let env_filter_stderr = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let env_filter_file = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let file_appender = RollingFileAppender::new(Rotation::DAILY, logs_dir, "relayer.log");

    let stderr_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(std::io::stderr)
        .with_filter(env_filter_stderr);

    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(file_appender)
        .with_filter(env_filter_file);

    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();

    Ok(())
}
