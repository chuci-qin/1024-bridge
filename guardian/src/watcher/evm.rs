use anyhow::{Result, Context};
use ethers::prelude::*;
use tracing::{info, error};
use tokio_stream::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::config::EvmChainConfig;
use crate::types::{Observation, CHAIN_ID_EVM};

// 定义 LogMessagePublished 事件
abigen!(
    CoreContract,
    r#"[
        event LogMessagePublished(address indexed sender, uint64 sequence, uint32 nonce, bytes payload, uint8 consistencyLevel)
    ]"#,
);

/// EVM chain watcher
pub struct EvmWatcher {
    config: EvmChainConfig,
    provider: Arc<Provider<Ws>>,
    contract_address: Address,
}

impl EvmWatcher {
    /// Create a new EVM watcher
    pub async fn new(config: &EvmChainConfig) -> Result<Self> {
        info!("Connecting to EVM RPC: {}", config.rpc_url);
        
        // Connect to WebSocket provider
        let provider = Provider::<Ws>::connect(&config.rpc_url)
            .await
            .context("Failed to connect to EVM WebSocket")?;
        
        // Parse contract address
        let contract_address: Address = config.core_contract.parse()
            .context("Invalid contract address")?;
        
        info!("✅ EVM watcher initialized for contract: {}", contract_address);
        
        Ok(Self {
            config: config.clone(),
            provider: Arc::new(provider),
            contract_address,
        })
    }
    
    /// Start watching for events and send to channel
    pub async fn watch_and_send(
        &mut self,
        obs_sender: mpsc::Sender<Observation>
    ) -> Result<()> {
        info!("Starting EVM event watcher...");
        
        // Create event filter
        let contract = CoreContract::new(self.contract_address, Arc::clone(&self.provider));
        let events = contract.log_message_published_filter();
        
        // Subscribe to events
        let mut stream = events.subscribe().await?;
        
        info!("✅ Subscribed to LogMessagePublished events");
        
        // Process events
        while let Some(event_result) = stream.next().await {
            match event_result {
                Ok(event) => {
                    info!(
                        "📨 New message: sender={:?}, sequence={}, nonce={}",
                        event.sender, event.sequence, event.nonce
                    );
                    
                    // Parse and handle observation
                    match self.parse_event(event).await {
                        Ok(observation) => {
                            info!("✅ Observation created: seq={}", observation.sequence);
                            
                            // Send to Guardian for signing
                            if let Err(e) = obs_sender.send(observation).await {
                                error!("Failed to send observation: {}", e);
                            }
                        }
                        Err(e) => {
                            error!("Failed to parse event: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Error receiving event: {}", e);
                }
            }
        }
        
        Ok(())
    }
    
    /// Parse LogMessagePublished event into Observation
    async fn parse_event(&self, log: LogMessagePublishedFilter) -> Result<Observation> {
        // Get current block for timestamp estimate
        let current_block = self.provider
            .get_block_number()
            .await?;
        
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as u32;
        
        // Convert sender address to 32 bytes
        let mut emitter_address = [0u8; 32];
        emitter_address[12..].copy_from_slice(log.sender.as_bytes());
        
        Ok(Observation {
            emitter_chain: CHAIN_ID_EVM,
            emitter_address,
            sequence: log.sequence,
            tx_hash: "pending".to_string(), // Will be updated when tx is mined
            block_number: current_block.as_u64(),
            timestamp,
            nonce: log.nonce,
            consistency_level: log.consistency_level,
            payload: log.payload.to_vec(),
        })
    }
}


            nonce: log.nonce,
            consistency_level: log.consistency_level,
            payload: log.payload.to_vec(),
        })
    }
}


            nonce: log.nonce,
            consistency_level: log.consistency_level,
            payload: log.payload.to_vec(),
        })
    }
}


            nonce: log.nonce,
            consistency_level: log.consistency_level,
            payload: log.payload.to_vec(),
        })
    }
}


            nonce: log.nonce,
            consistency_level: log.consistency_level,
            payload: log.payload.to_vec(),
        })
    }
}


            nonce: log.nonce,
            consistency_level: log.consistency_level,
            payload: log.payload.to_vec(),
        })
    }
}

