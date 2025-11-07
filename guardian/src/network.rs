/// 基于HTTP的简化P2P网络实现
/// Guardian间通过REST API相互通信

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::types::SignedObservation;

/// HTTP-based P2P network
pub struct P2PNetwork {
    /// My Guardian index
    my_index: u8,
    
    /// Other Guardian API URLs
    peer_urls: Vec<String>,
    
    /// HTTP client
    client: reqwest::Client,
}

impl P2PNetwork {
    /// Create new P2P network
    pub fn new(my_index: u8, peer_urls: Vec<String>) -> Self {
        info!("🌐 P2P Network initialized (HTTP mode)");
        info!("   Guardian {}: {} peers configured", my_index, peer_urls.len());
        
        Self {
            my_index,
            peer_urls,
            client: reqwest::Client::new(),
        }
    }
    
    /// Broadcast signed observation to all peers
    pub async fn broadcast_signature(&self, signed_obs: &SignedObservation) -> Result<()> {
        info!(
            "📡 Broadcasting signature from Guardian {} to {} peers",
            self.my_index,
            self.peer_urls.len()
        );
        
        let mut success_count = 0;
        
        for peer_url in &self.peer_urls {
            let url = format!("{}/v1/signature", peer_url);
            
            match self.client
                .post(&url)
                .json(signed_obs)
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    success_count += 1;
                }
                Ok(resp) => {
                    warn!("Peer {} returned {}", peer_url, resp.status());
                }
                Err(e) => {
                    warn!("Failed to send to {}: {}", peer_url, e);
                }
            }
        }
        
        info!("✅ Broadcasted to {}/{} peers", success_count, self.peer_urls.len());
        
        Ok(())
    }
    
    /// Poll signatures from peers (optional - for backup)
    pub async fn poll_peer_status(&self) -> Result<Vec<(String, bool)>> {
        let mut results = Vec::new();
        
        for peer_url in &self.peer_urls {
            let url = format!("{}/health", peer_url);
            
            match self.client
                .get(&url)
                .timeout(std::time::Duration::from_secs(2))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    results.push((peer_url.clone(), true));
                }
                _ => {
                    results.push((peer_url.clone(), false));
                }
            }
        }
        
        Ok(results)
    }
}

/// Generate peer URLs for local testing
pub fn generate_local_peer_urls(my_index: u8, total: u8) -> Vec<String> {
    (0..total)
        .filter(|&i| i != my_index)
        .map(|i| format!("http://localhost:{}", 7071 + i as u16))
        .collect()
}

