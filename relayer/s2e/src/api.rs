use crate::config::S2EConfig;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use shared::types::{HealthResponse, ServiceStatus};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower_http::cors::CorsLayer;
use tracing::info;

#[derive(Clone)]
struct AppState {
    config: S2EConfig,
    start_time: u64,
}

pub async fn start_server(config: S2EConfig) -> anyhow::Result<()> {
    let start_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs();

    let state = Arc::new(AppState {
        config: config.clone(),
        start_time,
    });

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/status", get(get_status))
        .route("/metrics", get(get_metrics))
        .with_state(state)
        .layer(CorsLayer::permissive());

    let addr = format!("0.0.0.0:{}", config.api.port);
    info!("HTTP API listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - state.start_time;

    let response = HealthResponse {
        status: "healthy".to_string(),
        service: state.config.service.name.clone(),
        version: state.config.service.version.clone(),
        uptime,
        timestamp: chrono::Utc::now(),
    };

    Json(response)
}

async fn get_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // 返回 relayer 状态
    let status = ServiceStatus {
        service: state.config.service.name.clone(),
        listening: true,
        source_chain: shared::types::ChainInfo {
            name: state.config.source_chain.name.clone(),
            chain_id: state.config.source_chain.chain_id,
            rpc: state.config.source_chain.rpc_url.clone(),
            connected: true,
            last_block: None,
        },
        target_chain: shared::types::ChainInfo {
            name: state.config.target_chain.name.clone(),
            chain_id: state.config.target_chain.chain_id,
            rpc: state.config.target_chain.rpc_url.clone(),
            connected: true,
            last_block: None,
        },
        relayer: shared::types::RelayerInfo {
            address: "unknown".to_string(),
            whitelisted: true,
            balance_svm: None,
            balance_evm: None,
        },
    };

    Json(status)
}

async fn get_metrics() -> impl IntoResponse {
    (StatusCode::OK, shared::metrics::export_metrics())
}
