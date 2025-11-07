use anyhow::Result;
use tracing::{info, error, warn};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

use crate::config::GuardianConfig;
use crate::types::{Observation, SignedObservation};
use crate::watcher::evm::EvmWatcher;
use crate::watcher::solana::SolanaWatcher;
use crate::signer::Signer;
use crate::aggregator::Aggregator;
use crate::network::P2PNetwork;

/// Guardian node with full integration
pub struct GuardianNode {
    config: GuardianConfig,
    signer: Signer,
    aggregator: Arc<RwLock<Aggregator>>,
    p2p: Arc<RwLock<Option<P2PNetwork>>>,
}

impl GuardianNode {
    /// Create a new Guardian node
    pub async fn new(config: GuardianConfig) -> Result<Self> {
        info!("Initializing Guardian node (index: {})...", config.guardian.index);
        
        // Create signer
        let signer = Signer::generate_random(config.guardian.index)?;
        info!("Guardian address: {:?}", hex::encode(signer.ethereum_address()));
        
        // Create aggregator
        let aggregator = Arc::new(RwLock::new(Aggregator::new(0)));
        
        // Create P2P network
        let peer_urls = crate::network::generate_local_peer_urls(config.guardian.index, 19);
        let p2p = P2PNetwork::new(config.guardian.index, peer_urls);
        
        Ok(Self {
            config,
            signer,
            aggregator,
            p2p: Arc::new(RwLock::new(Some(p2p))),
        })
    }
    
    /// Run the Guardian node
    pub async fn run(&mut self) -> Result<()> {
        info!("🚀 Guardian node starting...");
        
        // Channel for observations from watchers
        let (obs_tx, mut obs_rx) = mpsc::channel::<Observation>(100);
        
        // Start EVM watcher
        let evm_config = self.config.chains.evm.clone();
        let evm_tx = obs_tx.clone();
        tokio::spawn(async move {
            match EvmWatcher::new(&evm_config).await {
                Ok(mut watcher) => {
                    info!("✅ EVM Watcher started and integrated");
                    // Watch and send observations to main loop
                    if let Err(e) = watcher.watch_and_send(evm_tx).await {
                        error!("EVM Watcher error: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to start EVM watcher: {}", e);
                }
            }
        });
        
        // Start Solana watcher
        let solana_config = self.config.chains.solana.clone();
        let solana_tx = obs_tx.clone();
        tokio::spawn(async move {
            match SolanaWatcher::new(&solana_config).await {
                Ok(mut watcher) => {
                    info!("✅ Solana Watcher started and integrated");
                    // Watch and send observations to main loop
                    if let Err(e) = watcher.watch_and_send(solana_tx).await {
                        error!("Solana Watcher error: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to start Solana watcher: {}", e);
                }
            }
        });
        
        // Start API server
        let api_aggregator = Arc::clone(&self.aggregator);
        let api_listen = self.config.api.listen.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::api::start_server(&api_listen, api_aggregator).await {
                error!("API server error: {}", e);
            }
        });
        
        info!("✅ All services started");
        info!("   API: http://{}", self.config.api.listen);
        info!("   Guardian Index: {}", self.config.guardian.index);
        info!("   Watchers: EVM (WebSocket) + Solana (HTTP polling)");
        
        // Main event loop
        loop {
            tokio::select! {
                // Handle new observations
                Some(observation) = obs_rx.recv() => {
                    self.handle_observation(observation).await?;
                }
                
                // Keep alive
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(60)) => {
                    info!("Guardian heartbeat - still running");
                }
            }
        }
    }
    
    /// Handle a new observation
    async fn handle_observation(&mut self, observation: Observation) -> Result<()> {
        info!(
            "📨 New observation: chain={}, seq={}",
            observation.emitter_chain, observation.sequence
        );
        
        // Sign the observation
        let signature = self.signer.sign_observation(&observation)?;
        
        let signed_obs = SignedObservation {
            observation: observation.clone(),
            signature,
        };
        
        // Broadcast to other Guardians
        if let Some(p2p) = self.p2p.read().await.as_ref() {
            if let Err(e) = p2p.broadcast_signature(&signed_obs).await {
                warn!("Failed to broadcast: {}", e);
            }
        }
        
        // Add to local aggregator
        let mut aggregator = self.aggregator.write().await;
        aggregator.add_observation(observation);
        
        if let Some(_vaa) = aggregator.add_signature(signed_obs) {
            info!("🎉 VAA generated! (达到13/19 quorum)");
            // VAA is now available via API
        }
        
        Ok(())
    }
}


            "📨 New observation: chain={}, seq={}",
            observation.emitter_chain, observation.sequence
        );
        
        // Sign the observation
        let signature = self.signer.sign_observation(&observation)?;
        
        let signed_obs = SignedObservation {
            observation: observation.clone(),
            signature,
        };
        
        // Broadcast to other Guardians
        if let Some(p2p) = self.p2p.read().await.as_ref() {
            if let Err(e) = p2p.broadcast_signature(&signed_obs).await {
                warn!("Failed to broadcast: {}", e);
            }
        }
        
        // Add to local aggregator
        let mut aggregator = self.aggregator.write().await;
        aggregator.add_observation(observation);
        
        if let Some(_vaa) = aggregator.add_signature(signed_obs) {
            info!("🎉 VAA generated! (达到13/19 quorum)");
            // VAA is now available via API
        }
        
        Ok(())
    }
}


            "📨 New observation: chain={}, seq={}",
            observation.emitter_chain, observation.sequence
        );
        
        // Sign the observation
        let signature = self.signer.sign_observation(&observation)?;
        
        let signed_obs = SignedObservation {
            observation: observation.clone(),
            signature,
        };
        
        // Broadcast to other Guardians
        if let Some(p2p) = self.p2p.read().await.as_ref() {
            if let Err(e) = p2p.broadcast_signature(&signed_obs).await {
                warn!("Failed to broadcast: {}", e);
            }
        }
        
        // Add to local aggregator
        let mut aggregator = self.aggregator.write().await;
        aggregator.add_observation(observation);
        
        if let Some(_vaa) = aggregator.add_signature(signed_obs) {
            info!("🎉 VAA generated! (达到13/19 quorum)");
            // VAA is now available via API
        }
        
        Ok(())
    }
}


            "📨 New observation: chain={}, seq={}",
            observation.emitter_chain, observation.sequence
        );
        
        // Sign the observation
        let signature = self.signer.sign_observation(&observation)?;
        
        let signed_obs = SignedObservation {
            observation: observation.clone(),
            signature,
        };
        
        // Broadcast to other Guardians
        if let Some(p2p) = self.p2p.read().await.as_ref() {
            if let Err(e) = p2p.broadcast_signature(&signed_obs).await {
                warn!("Failed to broadcast: {}", e);
            }
        }
        
        // Add to local aggregator
        let mut aggregator = self.aggregator.write().await;
        aggregator.add_observation(observation);
        
        if let Some(_vaa) = aggregator.add_signature(signed_obs) {
            info!("🎉 VAA generated! (达到13/19 quorum)");
            // VAA is now available via API
        }
        
        Ok(())
    }
}


            "📨 New observation: chain={}, seq={}",
            observation.emitter_chain, observation.sequence
        );
        
        // Sign the observation
        let signature = self.signer.sign_observation(&observation)?;
        
        let signed_obs = SignedObservation {
            observation: observation.clone(),
            signature,
        };
        
        // Broadcast to other Guardians
        if let Some(p2p) = self.p2p.read().await.as_ref() {
            if let Err(e) = p2p.broadcast_signature(&signed_obs).await {
                warn!("Failed to broadcast: {}", e);
            }
        }
        
        // Add to local aggregator
        let mut aggregator = self.aggregator.write().await;
        aggregator.add_observation(observation);
        
        if let Some(_vaa) = aggregator.add_signature(signed_obs) {
            info!("🎉 VAA generated! (达到13/19 quorum)");
            // VAA is now available via API
        }
        
        Ok(())
    }
}


            "📨 New observation: chain={}, seq={}",
            observation.emitter_chain, observation.sequence
        );
        
        // Sign the observation
        let signature = self.signer.sign_observation(&observation)?;
        
        let signed_obs = SignedObservation {
            observation: observation.clone(),
            signature,
        };
        
        // Broadcast to other Guardians
        if let Some(p2p) = self.p2p.read().await.as_ref() {
            if let Err(e) = p2p.broadcast_signature(&signed_obs).await {
                warn!("Failed to broadcast: {}", e);
            }
        }
        
        // Add to local aggregator
        let mut aggregator = self.aggregator.write().await;
        aggregator.add_observation(observation);
        
        if let Some(_vaa) = aggregator.add_signature(signed_obs) {
            info!("🎉 VAA generated! (达到13/19 quorum)");
            // VAA is now available via API
        }
        
        Ok(())
    }
}

