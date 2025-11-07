use anyhow::{Result, bail};
use std::collections::HashMap;
use tracing::{info, warn};

use crate::types::{MessageId, Observation, Signature, SignedObservation, VAA, SIGNATURE_QUORUM};

/// VAA Aggregator - 收集签名并生成VAA
pub struct Aggregator {
    /// 当前 Guardian Set 索引
    guardian_set_index: u32,
    
    /// 签名收集器: MessageId -> Vec<Signature>
    signatures: HashMap<MessageId, Vec<Signature>>,
    
    /// 观察缓存: MessageId -> Observation
    observations: HashMap<MessageId, Observation>,
    
    /// 完成的VAAs
    completed_vaas: HashMap<MessageId, VAA>,
}

impl Aggregator {
    /// Create a new aggregator
    pub fn new(guardian_set_index: u32) -> Self {
        Self {
            guardian_set_index,
            signatures: HashMap::new(),
            observations: HashMap::new(),
            completed_vaas: HashMap::new(),
        }
    }
    
    /// Add an observation
    pub fn add_observation(&mut self, observation: Observation) {
        let message_id = MessageId::from(&observation);
        
        info!(
            "📝 Storing observation: chain={}, seq={}",
            observation.emitter_chain, observation.sequence
        );
        
        self.observations.insert(message_id, observation);
    }
    
    /// Add a signature
    pub fn add_signature(&mut self, signed_obs: SignedObservation) -> Option<VAA> {
        let message_id = MessageId::from(&signed_obs.observation);
        
        // Ensure we have the observation
        if !self.observations.contains_key(&message_id) {
            self.add_observation(signed_obs.observation.clone());
        }
        
        // Add signature
        let sigs = self.signatures.entry(message_id.clone()).or_insert_with(Vec::new);
        
        // Check if signature already exists (by guardian_index)
        if sigs.iter().any(|s| s.guardian_index == signed_obs.signature.guardian_index) {
            warn!(
                "Duplicate signature from guardian {}",
                signed_obs.signature.guardian_index
            );
            return None;
        }
        
        let guardian_idx = signed_obs.signature.guardian_index;
        sigs.push(signed_obs.signature);
        
        info!(
            "✍️  Added signature from guardian {}: {}/{} signatures",
            guardian_idx,
            sigs.len(),
            SIGNATURE_QUORUM
        );
        
        // Check if we reached quorum
        if sigs.len() >= SIGNATURE_QUORUM {
            info!("🎯 Quorum reached! Generating VAA...");
            return self.generate_vaa(&message_id);
        }
        
        None
    }
    
    /// Generate VAA when quorum is reached
    fn generate_vaa(&mut self, message_id: &MessageId) -> Option<VAA> {
        // Check if already generated
        if let Some(vaa) = self.completed_vaas.get(message_id) {
            return Some(vaa.clone());
        }
        
        let observation = self.observations.get(message_id)?;
        let signatures = self.signatures.get(message_id)?;
        
        if signatures.len() < SIGNATURE_QUORUM {
            return None;
        }
        
        // Sort signatures by guardian_index
        let mut sorted_sigs = signatures.clone();
        sorted_sigs.sort_by_key(|s| s.guardian_index);
        
        // Take first SIGNATURE_QUORUM signatures
        let final_sigs: Vec<Signature> = sorted_sigs.into_iter()
            .take(SIGNATURE_QUORUM)
            .collect();
        
        let vaa = VAA {
            version: 1,
            guardian_set_index: self.guardian_set_index,
            signatures: final_sigs,
            timestamp: observation.timestamp,
            nonce: observation.nonce,
            emitter_chain: observation.emitter_chain,
            emitter_address: observation.emitter_address,
            sequence: observation.sequence,
            consistency_level: observation.consistency_level,
            payload: observation.payload.clone(),
        };
        
        info!(
            "🎉 VAA generated: chain={}, seq={}, sigs={}",
            vaa.emitter_chain,
            vaa.sequence,
            vaa.signatures.len()
        );
        
        self.completed_vaas.insert(message_id.clone(), vaa.clone());
        
        Some(vaa)
    }
    
    /// Get completed VAA
    pub fn get_vaa(&self, message_id: &MessageId) -> Option<&VAA> {
        self.completed_vaas.get(message_id)
    }
    
    /// Get signature count for a message
    pub fn signature_count(&self, message_id: &MessageId) -> usize {
        self.signatures.get(message_id).map(|s| s.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::Signer;
    
    #[test]
    fn test_aggregator() {
        let mut aggregator = Aggregator::new(0);
        
        // Create test observation
        let observation = Observation {
            emitter_chain: 1,
            emitter_address: [1u8; 32],
            sequence: 100,
            tx_hash: "test".to_string(),
            block_number: 1000,
            timestamp: 1699264800,
            nonce: 12345,
            consistency_level: 200,
            payload: vec![1, 2, 3],
        };
        
        let message_id = MessageId::from(&observation);
        
        // Add observation
        aggregator.add_observation(observation.clone());
        
        // Add 13 signatures (quorum)
        for i in 0..13 {
            let signer = Signer::generate_random(i).unwrap();
            let signature = signer.sign_observation(&observation).unwrap();
            
            let signed_obs = SignedObservation {
                observation: observation.clone(),
                signature,
            };
            
            let vaa = aggregator.add_signature(signed_obs);
            
            if i < 12 {
                assert!(vaa.is_none(), "VAA should not be generated before quorum");
            } else {
                assert!(vaa.is_some(), "VAA should be generated at quorum");
                
                let vaa = vaa.unwrap();
                assert_eq!(vaa.signatures.len(), 13);
                assert_eq!(vaa.sequence, 100);
                assert_eq!(vaa.emitter_chain, 1);
            }
        }
        
        // Verify VAA is stored
        let vaa = aggregator.get_vaa(&message_id).unwrap();
        assert_eq!(vaa.signatures.len(), 13);
    }
    
    #[test]
    fn test_duplicate_signatures() {
        let mut aggregator = Aggregator::new(0);
        
        let observation = Observation {
            emitter_chain: 1,
            emitter_address: [1u8; 32],
            sequence: 200,
            tx_hash: "test".to_string(),
            block_number: 2000,
            timestamp: 1699264800,
            nonce: 54321,
            consistency_level: 200,
            payload: vec![4, 5, 6],
        };
        
        let signer = Signer::generate_random(0).unwrap();
        let signature = signer.sign_observation(&observation).unwrap();
        
        let signed_obs = SignedObservation {
            observation: observation.clone(),
            signature: signature.clone(),
        };
        
        // Add first time
        aggregator.add_signature(signed_obs.clone());
        assert_eq!(aggregator.signature_count(&MessageId::from(&observation)), 1);
        
        // Add duplicate
        aggregator.add_signature(signed_obs);
        assert_eq!(aggregator.signature_count(&MessageId::from(&observation)), 1);
    }
}

