use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::aggregator::Aggregator;
use crate::types::{MessageId, SignedObservation};

#[derive(Clone)]
pub struct ApiState {
    pub aggregator: Arc<RwLock<Aggregator>>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    guardian_index: Option<u8>,
    version: String,
}

#[derive(Serialize)]
struct VAAResponse {
    vaa_hex: String,
    emitter_chain: u16,
    emitter_address: String,
    sequence: u64,
    signatures_count: usize,
}

/// Create API router
pub fn create_router(aggregator: Arc<RwLock<Aggregator>>) -> Router {
    let state = ApiState { aggregator };

    Router::new()
        .route("/health", get(health))
        .route("/v1/signed_vaa/:chain/:emitter/:sequence", get(get_vaa))
        .route("/v1/signature", axum::routing::post(receive_signature))
        .with_state(state)
}

/// Receive signature from another Guardian
async fn receive_signature(
    State(state): State<ApiState>,
    Json(signed_obs): Json<SignedObservation>,
) -> impl IntoResponse {
    info!("📨 Received signature from Guardian {}", signed_obs.signature.guardian_index);
    
    // Add signature to aggregator
    let mut aggregator = state.aggregator.write().await;
    
    if let Some(_vaa) = aggregator.add_signature(signed_obs) {
        (StatusCode::OK, Json(serde_json::json!({"status": "vaa_ready"}))).into_response()
    } else {
        (StatusCode::ACCEPTED, Json(serde_json::json!({"status": "signature_added"}))).into_response()
    }
}

/// Health check endpoint
async fn health() -> impl IntoResponse {
    Json(HealthResponse {
        status: "healthy".to_string(),
        guardian_index: Some(0),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// Get VAA endpoint
async fn get_vaa(
    Path((chain_id, emitter_hex, sequence)): Path<(u16, String, u64)>,
    State(state): State<ApiState>,
) -> impl IntoResponse {
    // Parse emitter address
    let emitter_bytes = match hex::decode(emitter_hex.trim_start_matches("0x")) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&bytes);
            addr
        }
        Ok(bytes) if bytes.len() == 20 => {
            // Ethereum address (20 bytes) -> pad to 32 bytes
            let mut addr = [0u8; 32];
            addr[12..].copy_from_slice(&bytes);
            addr
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "Invalid emitter address"
                }))
            ).into_response();
        }
    };

    let message_id = MessageId {
        emitter_chain: chain_id,
        emitter_address: emitter_bytes,
        sequence,
    };

    // Get VAA from aggregator
    let aggregator = state.aggregator.read().await;
    
    match aggregator.get_vaa(&message_id) {
        Some(vaa) => {
            // Serialize VAA to hex
            let vaa_bytes = serialize_vaa(vaa);
            
            (
                StatusCode::OK,
                Json(VAAResponse {
                    vaa_hex: format!("0x{}", hex::encode(&vaa_bytes)),
                    emitter_chain: vaa.emitter_chain,
                    emitter_address: format!("0x{}", hex::encode(&vaa.emitter_address)),
                    sequence: vaa.sequence,
                    signatures_count: vaa.signatures.len(),
                })
            ).into_response()
        }
        None => {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "VAA not found",
                    "message_id": format!("{}/{}/{}", chain_id, hex::encode(&emitter_bytes), sequence)
                }))
            ).into_response()
        }
    }
}

/// Serialize VAA to bytes
fn serialize_vaa(vaa: &crate::types::VAA) -> Vec<u8> {
    let mut bytes = Vec::new();
    
    // Header
    bytes.push(vaa.version);
    bytes.extend_from_slice(&vaa.guardian_set_index.to_be_bytes());
    bytes.push(vaa.signatures.len() as u8);
    
    // Signatures
    for sig in &vaa.signatures {
        bytes.push(sig.guardian_index);
        bytes.extend_from_slice(&sig.r);
        bytes.extend_from_slice(&sig.s);
        bytes.push(sig.v);
    }
    
    // Body
    bytes.extend_from_slice(&vaa.timestamp.to_be_bytes());
    bytes.extend_from_slice(&vaa.nonce.to_be_bytes());
    bytes.extend_from_slice(&vaa.emitter_chain.to_be_bytes());
    bytes.extend_from_slice(&vaa.emitter_address);
    bytes.extend_from_slice(&vaa.sequence.to_be_bytes());
    bytes.push(vaa.consistency_level);
    bytes.extend_from_slice(&vaa.payload);
    
    bytes
}

/// Start API server
pub async fn start_server(
    listen_addr: &str,
    aggregator: Arc<RwLock<Aggregator>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = create_router(aggregator);
    
    info!("🌐 Starting API server on {}", listen_addr);
    
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}

