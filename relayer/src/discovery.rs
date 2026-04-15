use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use solana_account_decoder::UiAccountEncoding;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use tracing::{info, warn};

use crate::chain_registry::{get_chain_info, resolve_rpc};
use crate::types::PeerInfo;

#[derive(Debug)]
pub struct BridgeStateInfo {
    pub local_chain_id: u64,
    pub usdc_mint: Pubkey,
    pub relayers: Vec<Pubkey>,
}

fn anchor_discriminator(account_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(format!("account:{account_name}"));
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

/// Fetch BridgeState PDA from the 1024 chain.
pub fn fetch_bridge_state(rpc: &RpcClient, program_id: &Pubkey) -> Result<BridgeStateInfo> {
    let (bridge_state_pda, _) = Pubkey::find_program_address(&[b"bridge_state"], program_id);

    let account = rpc
        .get_account_with_commitment(&bridge_state_pda, CommitmentConfig::finalized())?
        .value
        .with_context(|| "BridgeState PDA account not found")?;

    let data = &account.data;
    let disc = anchor_discriminator("BridgeState");
    if data.len() < 8 || data[..8] != disc {
        bail!("BridgeState discriminator mismatch");
    }

    // Skip past fixed-size fields to reach the ones we need.
    // Layout after 8-byte discriminator:
    //   admin(32) + guardian(32) + operator(32) + recovery(32) + pending_admin(32) = 160
    //   vault_bump(1)
    //   usdc_mint(32)
    //   local_chain_id(8)
    //   7 × u64 rate limits (56)
    //   is_paused(1) + timelock_active(1)
    //   relayers: Vec<Pubkey>
    let mut offset = 8 + 32 * 5 + 1; // past discriminator + 5 pubkeys + vault_bump

    let usdc_mint = Pubkey::try_from(&data[offset..offset + 32])
        .map_err(|e| anyhow::anyhow!("parse usdc_mint: {e:?}"))?;
    offset += 32;

    let local_chain_id = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
    offset += 8;

    offset += 8 * 7; // rate limit fields
    offset += 2; // is_paused + timelock_active

    if offset + 4 > data.len() {
        bail!("BridgeState data too short for relayers length");
    }
    let relayer_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    let mut relayers = Vec::with_capacity(relayer_count);
    for _ in 0..relayer_count {
        if offset + 32 > data.len() {
            bail!("BridgeState data too short for relayer pubkey");
        }
        let pk = Pubkey::try_from(&data[offset..offset + 32])
            .map_err(|e| anyhow::anyhow!("parse relayer: {e:?}"))?;
        relayers.push(pk);
        offset += 32;
    }

    Ok(BridgeStateInfo {
        local_chain_id,
        usdc_mint,
        relayers,
    })
}

/// Discover all PeerConfig PDAs via getProgramAccounts with discriminator filter.
pub fn discover_peers(rpc: &RpcClient, program_id: &Pubkey) -> Result<Vec<PeerInfo>> {
    let disc = anchor_discriminator("PeerConfig");

    let filters = vec![RpcFilterType::Memcmp(Memcmp::new_raw_bytes(
        0,
        disc.to_vec(),
    ))];

    let config = RpcProgramAccountsConfig {
        filters: Some(filters),
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            commitment: Some(CommitmentConfig::finalized()),
            ..Default::default()
        },
        ..Default::default()
    };

    let accounts = rpc.get_program_accounts_with_config(program_id, config)?;

    let mut peers = Vec::new();
    for (_pubkey, account) in &accounts {
        let data = &account.data;
        if data.len() < 8 + 8 + 32 {
            warn!("Skipping PeerConfig account with insufficient data");
            continue;
        }

        let chain_id = u64::from_le_bytes(data[8..16].try_into()?);
        let mut peer_contract = [0u8; 32];
        peer_contract.copy_from_slice(&data[16..48]);

        let chain_info = match get_chain_info(chain_id) {
            Some(ci) => ci,
            None => {
                warn!(chain_id, "Peer chain_id not in registry, skipping");
                continue;
            }
        };

        let rpc_url = resolve_rpc(chain_info);

        peers.push(PeerInfo {
            chain_id,
            peer_contract,
            kind: chain_info.kind,
            rpc_url,
        });

        info!(
            chain_id,
            kind = %chain_info.kind,
            env_name = chain_info.env_name,
            "Discovered peer"
        );
    }

    Ok(peers)
}
