use anyhow::{Result, Context};
use tokio::sync::mpsc;
use tracing::{info, error};
use serde::{Deserialize, Serialize};

use crate::config::SolanaChainConfig;
use crate::types::{Observation, CHAIN_ID_SOLANA};

#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: T,
}

#[derive(Debug, Deserialize, Serialize)]
struct SignatureStatus {
    slot: u64,
}

/// Solana chain watcher using HTTP RPC polling
pub struct SolanaWatcher {
    config: SolanaChainConfig,
    program_id: String,
    client: reqwest::Client,
    last_slot: u64,
}

impl SolanaWatcher {
    /// Create a new Solana watcher
    pub async fn new(config: &SolanaChainConfig) -> Result<Self> {
        info!("Initializing Solana watcher (HTTP polling mode)");
        info!("  RPC: {}", config.rpc_url);
        info!("  Program: {}", config.core_program);
        
        Ok(Self {
            config: config.clone(),
            program_id: config.core_program.clone(),
            client: reqwest::Client::new(),
            last_slot: 0,
        })
    }
    
    /// Start watching for events and send to channel
    pub async fn watch_and_send(
        &mut self,
        obs_sender: mpsc::Sender<Observation>
    ) -> Result<()> {
        info!("✅ Solana watcher started (HTTP polling every 2s)");
        
        loop {
            // Poll for new signatures involving our program
            match self.check_new_messages().await {
                Ok(observations) => {
                    for obs in observations {
                        info!(
                            "📨 Solana message: slot={}, seq={}",
                            obs.block_number, obs.sequence
                        );
                        
                        if let Err(e) = obs_sender.send(obs).await {
                            error!("Failed to send observation: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to check messages: {}", e);
                }
            }
            
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }
    
    /// Check for new messages via RPC
    async fn check_new_messages(&mut self) -> Result<Vec<Observation>> {
        // Get current slot
        let slot = self.get_slot().await?;
        
        if slot > self.last_slot {
            // New slot, check for messages
            // In production, would query program accounts
            self.last_slot = slot;
        }
        
        Ok(vec![])
    }
    
    /// Get current slot via RPC
    async fn get_slot(&self) -> Result<u64> {
        let response = self.client
            .post(&self.config.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSlot"
            }))
            .send()
            .await?;
        
        let data: RpcResponse<u64> = response.json().await?;
        Ok(data.result)
    }
}



        Ok(Self {
            config: config.clone(),
            program_id: config.core_program.clone(),
            client: reqwest::Client::new(),
            last_slot: 0,
        })
    }
    
    /// Start watching for events and send to channel
    pub async fn watch_and_send(
        &mut self,
        obs_sender: mpsc::Sender<Observation>
    ) -> Result<()> {
        info!("✅ Solana watcher started (HTTP polling every 2s)");
        
        loop {
            // Poll for new signatures involving our program
            match self.check_new_messages().await {
                Ok(observations) => {
                    for obs in observations {
                        info!(
                            "📨 Solana message: slot={}, seq={}",
                            obs.block_number, obs.sequence
                        );
                        
                        if let Err(e) = obs_sender.send(obs).await {
                            error!("Failed to send observation: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to check messages: {}", e);
                }
            }
            
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }
    
    /// Check for new messages via RPC
    async fn check_new_messages(&mut self) -> Result<Vec<Observation>> {
        // Get current slot
        let slot = self.get_slot().await?;
        
        if slot > self.last_slot {
            // New slot, check for messages
            // In production, would query program accounts
            self.last_slot = slot;
        }
        
        Ok(vec![])
    }
    
    /// Get current slot via RPC
    async fn get_slot(&self) -> Result<u64> {
        let response = self.client
            .post(&self.config.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSlot"
            }))
            .send()
            .await?;
        
        let data: RpcResponse<u64> = response.json().await?;
        Ok(data.result)
    }
}



        Ok(Self {
            config: config.clone(),
            program_id: config.core_program.clone(),
            client: reqwest::Client::new(),
            last_slot: 0,
        })
    }
    
    /// Start watching for events and send to channel
    pub async fn watch_and_send(
        &mut self,
        obs_sender: mpsc::Sender<Observation>
    ) -> Result<()> {
        info!("✅ Solana watcher started (HTTP polling every 2s)");
        
        loop {
            // Poll for new signatures involving our program
            match self.check_new_messages().await {
                Ok(observations) => {
                    for obs in observations {
                        info!(
                            "📨 Solana message: slot={}, seq={}",
                            obs.block_number, obs.sequence
                        );
                        
                        if let Err(e) = obs_sender.send(obs).await {
                            error!("Failed to send observation: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to check messages: {}", e);
                }
            }
            
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }
    
    /// Check for new messages via RPC
    async fn check_new_messages(&mut self) -> Result<Vec<Observation>> {
        // Get current slot
        let slot = self.get_slot().await?;
        
        if slot > self.last_slot {
            // New slot, check for messages
            // In production, would query program accounts
            self.last_slot = slot;
        }
        
        Ok(vec![])
    }
    
    /// Get current slot via RPC
    async fn get_slot(&self) -> Result<u64> {
        let response = self.client
            .post(&self.config.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSlot"
            }))
            .send()
            .await?;
        
        let data: RpcResponse<u64> = response.json().await?;
        Ok(data.result)
    }
}



        Ok(Self {
            config: config.clone(),
            program_id: config.core_program.clone(),
            client: reqwest::Client::new(),
            last_slot: 0,
        })
    }
    
    /// Start watching for events and send to channel
    pub async fn watch_and_send(
        &mut self,
        obs_sender: mpsc::Sender<Observation>
    ) -> Result<()> {
        info!("✅ Solana watcher started (HTTP polling every 2s)");
        
        loop {
            // Poll for new signatures involving our program
            match self.check_new_messages().await {
                Ok(observations) => {
                    for obs in observations {
                        info!(
                            "📨 Solana message: slot={}, seq={}",
                            obs.block_number, obs.sequence
                        );
                        
                        if let Err(e) = obs_sender.send(obs).await {
                            error!("Failed to send observation: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to check messages: {}", e);
                }
            }
            
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }
    
    /// Check for new messages via RPC
    async fn check_new_messages(&mut self) -> Result<Vec<Observation>> {
        // Get current slot
        let slot = self.get_slot().await?;
        
        if slot > self.last_slot {
            // New slot, check for messages
            // In production, would query program accounts
            self.last_slot = slot;
        }
        
        Ok(vec![])
    }
    
    /// Get current slot via RPC
    async fn get_slot(&self) -> Result<u64> {
        let response = self.client
            .post(&self.config.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSlot"
            }))
            .send()
            .await?;
        
        let data: RpcResponse<u64> = response.json().await?;
        Ok(data.result)
    }
}



        Ok(Self {
            config: config.clone(),
            program_id: config.core_program.clone(),
            client: reqwest::Client::new(),
            last_slot: 0,
        })
    }
    
    /// Start watching for events and send to channel
    pub async fn watch_and_send(
        &mut self,
        obs_sender: mpsc::Sender<Observation>
    ) -> Result<()> {
        info!("✅ Solana watcher started (HTTP polling every 2s)");
        
        loop {
            // Poll for new signatures involving our program
            match self.check_new_messages().await {
                Ok(observations) => {
                    for obs in observations {
                        info!(
                            "📨 Solana message: slot={}, seq={}",
                            obs.block_number, obs.sequence
                        );
                        
                        if let Err(e) = obs_sender.send(obs).await {
                            error!("Failed to send observation: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to check messages: {}", e);
                }
            }
            
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }
    
    /// Check for new messages via RPC
    async fn check_new_messages(&mut self) -> Result<Vec<Observation>> {
        // Get current slot
        let slot = self.get_slot().await?;
        
        if slot > self.last_slot {
            // New slot, check for messages
            // In production, would query program accounts
            self.last_slot = slot;
        }
        
        Ok(vec![])
    }
    
    /// Get current slot via RPC
    async fn get_slot(&self) -> Result<u64> {
        let response = self.client
            .post(&self.config.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSlot"
            }))
            .send()
            .await?;
        
        let data: RpcResponse<u64> = response.json().await?;
        Ok(data.result)
    }
}



        Ok(Self {
            config: config.clone(),
            program_id: config.core_program.clone(),
            client: reqwest::Client::new(),
            last_slot: 0,
        })
    }
    
    /// Start watching for events and send to channel
    pub async fn watch_and_send(
        &mut self,
        obs_sender: mpsc::Sender<Observation>
    ) -> Result<()> {
        info!("✅ Solana watcher started (HTTP polling every 2s)");
        
        loop {
            // Poll for new signatures involving our program
            match self.check_new_messages().await {
                Ok(observations) => {
                    for obs in observations {
                        info!(
                            "📨 Solana message: slot={}, seq={}",
                            obs.block_number, obs.sequence
                        );
                        
                        if let Err(e) = obs_sender.send(obs).await {
                            error!("Failed to send observation: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to check messages: {}", e);
                }
            }
            
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }
    
    /// Check for new messages via RPC
    async fn check_new_messages(&mut self) -> Result<Vec<Observation>> {
        // Get current slot
        let slot = self.get_slot().await?;
        
        if slot > self.last_slot {
            // New slot, check for messages
            // In production, would query program accounts
            self.last_slot = slot;
        }
        
        Ok(vec![])
    }
    
    /// Get current slot via RPC
    async fn get_slot(&self) -> Result<u64> {
        let response = self.client
            .post(&self.config.rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSlot"
            }))
            .send()
            .await?;
        
        let data: RpcResponse<u64> = response.json().await?;
        Ok(data.result)
    }
}


