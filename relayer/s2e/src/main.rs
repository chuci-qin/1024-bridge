mod api;
mod config;
mod listener;
mod signer;
mod submitter;

use anyhow::Result;
use shared::{logger, metrics};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // 加载配置
    let config = config::load_s2e_config()?;
    
    // 初始化日志（默认输出到 ./logs/s2e.log）
    let log_file = config.logging.log_file.clone()
        .unwrap_or_else(|| "./logs/s2e.log".to_string());
    let _log_guard = logger::init_logger_with_file(
        &config.logging.level,
        &config.logging.format,
        Some(&log_file),
    )?;
    info!("Starting s2e relayer service");
    info!(
        source = config.source_chain.name,
        target = config.target_chain.name,
        "Service configuration loaded"
    );

    // 初始化指标
    metrics::init_metrics();

    // 验证配置
    config.validate()?;
    info!("Configuration validated");

    // 启动 HTTP API 服务器（不阻塞主流程）
    let api_config = config.clone();
    tokio::spawn(async move {
        match api::start_server(api_config).await {
            Ok(_) => info!("API server stopped gracefully"),
            Err(e) => tracing::error!("API server error: {}", e),
        }
    });
    info!(port = config.api.port, "HTTP API server started");

    // 启动事件监听器（主要服务）
    info!("Event listener started");

    // 等待服务（只等待 listener 和 Ctrl-C）
    tokio::select! {
        result = listener::start_listener(config) => {
            if let Err(e) = result {
                tracing::error!("Event listener returned error: {}", e);
            }
            info!("Event listener stopped");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
    }

    info!("s2e relayer service stopped");
    Ok(())
}
