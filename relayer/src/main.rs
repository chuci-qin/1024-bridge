mod chain_registry;
mod checkpoint;
mod config;
mod discovery;
mod error;
mod evm;
mod keys;
mod logging;
mod svm;
mod types;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ethers::providers::{Http, Provider};
use ethers::signers::LocalWallet;
use ethers::types::Address;
use rand::Rng;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::signer::Signer;
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::checkpoint::*;
use crate::config::Config;
use crate::types::*;

type EvtHandle = tokio::task::JoinHandle<(StakeEventData, bool)>;

/// Spawn a task that confirms a single event on an SVM target chain.
fn spawn_svm_confirm(
    rpc: Arc<RpcClient>,
    program_id: Pubkey,
    usdc_mint: Pubkey,
    token_program_id: Pubkey,
    kp_bytes: [u8; 64],
    event: StakeEventData,
    peer_chain_id: u64,
    direction: Direction,
) -> EvtHandle {
    tokio::spawn(async move {
        let kp = solana_sdk::signature::Keypair::try_from(kp_bytes.as_slice()).expect("keypair");
        let ok = process_event_for_svm(
            &rpc, &program_id, &usdc_mint, &token_program_id, &kp,
            &event, peer_chain_id, direction,
        ).await;
        (event, ok)
    })
}

/// Spawn a task that confirms a single event on an EVM target chain.
fn spawn_evm_confirm(
    wallet: LocalWallet,
    provider: Provider<Http>,
    contract_address: Address,
    chain_id: u64,
    event: StakeEventData,
    direction: Direction,
) -> EvtHandle {
    tokio::spawn(async move {
        let ok = process_event_for_evm(
            &wallet, &provider, contract_address,
            chain_id, &event, direction,
        ).await;
        (event, ok)
    })
}

/// Await all spawned tasks, return events that failed (for retry).
async fn collect_failures(handles: Vec<EvtHandle>) -> Vec<StakeEventData> {
    let mut failed = Vec::new();
    for r in futures::future::join_all(handles).await {
        match r {
            Ok((_, true)) => {}
            Ok((event, false)) => failed.push(event),
            Err(e) => warn!("Event processing task panicked: {e}"),
        }
    }
    failed
}

const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Short delay between calls while catching up, to avoid rate-limiting.
const CATCHUP_DELAY: Duration = Duration::from_millis(200);
/// Max blocks per eth_getLogs call. Alchemy free tier caps at 10 blocks.
const EVM_BLOCK_RANGE: u64 = 10;
/// How many blocks to scan back on EVM when no checkpoint exists.
const EVM_INITIAL_SCAN_BACK: u64 = 1000;
/// Signatures fetched per getSignaturesForAddress RPC call (pagination page size).
const SVM_SIG_BATCH: usize = 50;
/// Max total signatures to accumulate across pages in a single poll cycle.
const SVM_MAX_SIGS: usize = 1000;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_env()?;
    config.ensure_dirs()?;

    logging::init(&config.logs_dir())?;

    info!(
        network = %config.network,
        chain_id = config.chain_1024_id,
        rpc = %config.chain_1024_rpc,
        "Starting Bridge1024 relayer"
    );

    let keys = keys::Keys::load_or_generate(&config.keys_dir())?;
    let svm_pubkey = keys.svm_keypair.pubkey();

    info!(svm_pubkey = %svm_pubkey, "Relayer keys loaded");

    let program_id = Pubkey::from_str(&config.bridge_program_id)
        .context("Invalid BRIDGE_1024_PROGRAM_ID")?;

    let rpc_1024 = RpcClient::new_with_commitment(
        config.chain_1024_rpc.clone(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );

    info!("Fetching BridgeState from 1024 chain...");
    let bridge_state = discovery::fetch_bridge_state(&rpc_1024, &program_id)?;

    info!(
        local_chain_id = bridge_state.local_chain_id,
        relayer_count = bridge_state.relayers.len(),
        "BridgeState loaded"
    );

    if !bridge_state.relayers.contains(&svm_pubkey) {
        warn!("Our SVM pubkey is NOT in the bridge relayers list -- confirmations will fail until whitelisted");
    }

    info!("Discovering peers...");
    let peers = discovery::discover_peers(&rpc_1024, &program_id)?;

    if peers.is_empty() {
        bail!("No peers discovered -- nothing to relay");
    }

    info!(peer_count = peers.len(), "Peer discovery complete");

    let usdc_mint = bridge_state.usdc_mint;
    let token_program_id = {
        let mint_account = rpc_1024
            .get_account(&usdc_mint)
            .context("fetch USDC mint account to detect token program")?;
        info!(
            usdc_mint = %usdc_mint,
            token_program = %mint_account.owner,
            "Detected USDC token program"
        );
        mint_account.owner
    };

    let config = Arc::new(config);

    let mut handles = Vec::new();

    // Per-peer inbound tasks: each polls its own peer chain → confirms on 1024
    for peer in &peers {
        let peer = peer.clone();
        let config = Arc::clone(&config);
        let program_id = program_id;
        let rpc_url_1024 = config.chain_1024_rpc.clone();
        let usdc_mint = usdc_mint;
        let token_program_id = token_program_id;
        let svm_keypair_bytes = keys.svm_keypair.to_bytes().to_vec();

        let handle = tokio::spawn(async move {
            let keypair = solana_sdk::signature::Keypair::try_from(svm_keypair_bytes.as_slice())
                .expect("reconstruct keypair");
            if let Err(e) = run_inbound_task(
                &config,
                &peer,
                &program_id,
                &rpc_url_1024,
                &usdc_mint,
                &token_program_id,
                &keypair,
            )
            .await
            {
                error!(
                    chain_id = peer.chain_id,
                    direction = "inbound",
                    "Inbound task failed: {e:#}"
                );
            }
        });
        handles.push(handle);
    }

    // Single outbound task: polls 1024 chain once, dispatches to all peers
    {
        let config = Arc::clone(&config);
        let peers = peers.clone();
        let program_id = program_id;
        let rpc_url_1024 = config.chain_1024_rpc.clone();
        let usdc_mint = usdc_mint;
        let token_program_id = token_program_id;
        let evm_wallet = keys.evm_wallet.clone();
        let svm_keypair_bytes = keys.svm_keypair.to_bytes().to_vec();

        let handle = tokio::spawn(async move {
            let keypair = solana_sdk::signature::Keypair::try_from(svm_keypair_bytes.as_slice())
                .expect("reconstruct keypair");
            if let Err(e) = run_outbound_poller(
                &config,
                &peers,
                &program_id,
                &rpc_url_1024,
                &usdc_mint,
                &token_program_id,
                &evm_wallet,
                &keypair,
            )
            .await
            {
                error!("Outbound poller failed: {e:#}");
            }
        });
        handles.push(handle);
    }

    futures::future::join_all(handles).await;
    Ok(())
}

/// Inbound: poll peer chain for StakeEvents → submit confirm on 1024 chain (SVM).
async fn run_inbound_task(
    config: &Config,
    peer: &PeerInfo,
    program_id: &Pubkey,
    rpc_url_1024: &str,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    relayer_keypair: &solana_sdk::signature::Keypair,
) -> Result<()> {
    let checkpoints_dir = config.checkpoints_dir();

    info!(
        chain_id = peer.chain_id,
        kind = %peer.kind,
        direction = "inbound",
        "Starting inbound poller"
    );

    match peer.kind {
        ChainKind::Evm => {
            run_inbound_evm(
                &checkpoints_dir,
                peer,
                program_id,
                rpc_url_1024,
                usdc_mint,
                token_program_id,
                relayer_keypair,
            )
            .await
        }
        ChainKind::Svm => {
            run_inbound_svm(
                &checkpoints_dir,
                peer,
                program_id,
                rpc_url_1024,
                usdc_mint,
                token_program_id,
                relayer_keypair,
            )
            .await
        }
    }
}

async fn run_inbound_evm(
    checkpoints_dir: &std::path::Path,
    peer: &PeerInfo,
    program_id: &Pubkey,
    rpc_url_1024: &str,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    relayer_keypair: &solana_sdk::signature::Keypair,
) -> Result<()> {
    let provider = Provider::<Http>::try_from(&peer.rpc_url)
        .context("create EVM provider")?;
    let target_rpc = Arc::new(RpcClient::new_with_commitment(
        rpc_url_1024.to_string(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    ));
    let kp_bytes = relayer_keypair.to_bytes();

    let contract_address = bytes32_to_evm_address(&peer.peer_contract)?;

    let mut from_block = match load_evm_checkpoint(checkpoints_dir, Direction::Inbound, peer.chain_id)? {
        Some(cp) => cp.last_block,
        None => {
            let start = evm::poller::initial_from_block(&provider, EVM_INITIAL_SCAN_BACK).await?;
            info!(chain_id = peer.chain_id, from_block = start, "No checkpoint, scanning back recent blocks");
            start
        }
    };

    let mut pending_retry: Vec<StakeEventData> = Vec::new();

    loop {
        let mut catching_up = false;
        let mut handles: Vec<EvtHandle> = Vec::new();

        // Pre-filter retries: drop already-processed nonces (sync check)
        pending_retry.retain(|event| {
            match svm::submitter::check_nonce_processed(&target_rpc, program_id, event.source_chain_id, event.nonce) {
                Ok(true) => false,
                _ => true,
            }
        });

        // Spawn retries in parallel
        for event in pending_retry.drain(..) {
            handles.push(spawn_svm_confirm(
                Arc::clone(&target_rpc), *program_id, *usdc_mint, *token_program_id,
                kp_bytes, event, peer.chain_id, Direction::Inbound,
            ));
        }

        // Poll new events and spawn in parallel
        match evm::poller::poll_evm_events(&provider, contract_address, from_block, EVM_BLOCK_RANGE).await {
            Ok((events, new_from)) => {
                for event in events {
                    handles.push(spawn_svm_confirm(
                        Arc::clone(&target_rpc), *program_id, *usdc_mint, *token_program_id,
                        kp_bytes, event, peer.chain_id, Direction::Inbound,
                    ));
                }

                if new_from > from_block {
                    catching_up = (new_from - from_block) > EVM_BLOCK_RANGE;
                    from_block = new_from;
                    let cp = EvmCheckpoint { last_block: from_block };
                    if let Err(e) = save_evm_checkpoint(checkpoints_dir, Direction::Inbound, peer.chain_id, &cp) {
                        warn!(chain_id = peer.chain_id, "Failed to save checkpoint: {e}");
                    }
                }
            }
            Err(e) => {
                warn!(chain_id = peer.chain_id, "EVM poll error: {e}");
            }
        }

        pending_retry.extend(collect_failures(handles).await);

        if catching_up {
            sleep(CATCHUP_DELAY).await;
        } else {
            sleep(POLL_INTERVAL).await;
        }
    }
}

async fn run_inbound_svm(
    checkpoints_dir: &std::path::Path,
    peer: &PeerInfo,
    program_id: &Pubkey,
    rpc_url_1024: &str,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    relayer_keypair: &solana_sdk::signature::Keypair,
) -> Result<()> {
    let peer_rpc = RpcClient::new_with_commitment(
        peer.rpc_url.clone(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );
    let target_rpc = Arc::new(RpcClient::new_with_commitment(
        rpc_url_1024.to_string(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    ));
    let kp_bytes = relayer_keypair.to_bytes();

    let peer_program_id = Pubkey::new_from_array(peer.peer_contract);

    let mut last_sig = match load_svm_checkpoint(checkpoints_dir, Direction::Inbound, peer.chain_id)? {
        Some(cp) => {
            Some(Signature::from_str(&cp.last_signature).context("parse saved signature")?)
        }
        None => {
            info!(chain_id = peer.chain_id, max_sigs = SVM_MAX_SIGS, "No checkpoint, scanning recent signatures");
            None
        }
    };

    let mut pending_retry: Vec<StakeEventData> = Vec::new();

    loop {
        let mut handles: Vec<EvtHandle> = Vec::new();

        pending_retry.retain(|event| {
            match svm::submitter::check_nonce_processed(&target_rpc, program_id, event.source_chain_id, event.nonce) {
                Ok(true) => false,
                _ => true,
            }
        });
        for event in pending_retry.drain(..) {
            handles.push(spawn_svm_confirm(
                Arc::clone(&target_rpc), *program_id, *usdc_mint, *token_program_id,
                kp_bytes, event, peer.chain_id, Direction::Inbound,
            ));
        }

        match svm::poller::poll_svm_events(&peer_rpc, &peer_program_id, last_sig.as_ref(), SVM_SIG_BATCH, SVM_MAX_SIGS) {
            Ok((events, newest_sig)) => {
                for event in events {
                    handles.push(spawn_svm_confirm(
                        Arc::clone(&target_rpc), *program_id, *usdc_mint, *token_program_id,
                        kp_bytes, event, peer.chain_id, Direction::Inbound,
                    ));
                }

                if let Some(sig) = newest_sig {
                    last_sig = Some(sig);
                    let cp = SvmCheckpoint {
                        last_signature: sig.to_string(),
                    };
                    if let Err(e) = save_svm_checkpoint(checkpoints_dir, Direction::Inbound, peer.chain_id, &cp) {
                        warn!(chain_id = peer.chain_id, "Failed to save checkpoint: {e}");
                    }
                }
            }
            Err(e) => {
                warn!(chain_id = peer.chain_id, "SVM poll error: {e}");
            }
        }

        pending_retry.extend(collect_failures(handles).await);

        sleep(POLL_INTERVAL).await;
    }
}

/// Per-peer context for outbound event submission.
struct OutboundPeerCtx {
    peer: PeerInfo,
    evm_provider: Option<Provider<Http>>,
    evm_address: Option<Address>,
    svm_rpc: Option<Arc<RpcClient>>,
    pending_retry: Vec<StakeEventData>,
}

impl OutboundPeerCtx {
    /// Spawn a confirm task for one event, choosing EVM or SVM path based on peer kind.
    fn spawn_confirm(
        &self,
        evm_wallet: &LocalWallet,
        usdc_mint: &Pubkey,
        token_program_id: &Pubkey,
        kp_bytes: [u8; 64],
        event: StakeEventData,
    ) -> EvtHandle {
        match self.peer.kind {
            ChainKind::Evm => spawn_evm_confirm(
                evm_wallet.clone(),
                self.evm_provider.clone().expect("EVM provider for EVM peer"),
                self.evm_address.expect("EVM address for EVM peer"),
                self.peer.chain_id,
                event,
                Direction::Outbound,
            ),
            ChainKind::Svm => spawn_svm_confirm(
                Arc::clone(self.svm_rpc.as_ref().expect("SVM RPC for SVM peer")),
                Pubkey::new_from_array(self.peer.peer_contract),
                *usdc_mint,
                *token_program_id,
                kp_bytes,
                event,
                self.peer.chain_id,
                Direction::Outbound,
            ),
        }
    }
}

/// Single outbound poller: polls 1024 chain once per cycle, dispatches events to all peers in parallel.
async fn run_outbound_poller(
    config: &Config,
    peers: &[PeerInfo],
    program_id: &Pubkey,
    rpc_url_1024: &str,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    evm_wallet: &LocalWallet,
    svm_keypair: &solana_sdk::signature::Keypair,
) -> Result<()> {
    let checkpoints_dir = config.checkpoints_dir();
    let kp_bytes = svm_keypair.to_bytes();

    let rpc_1024 = RpcClient::new_with_commitment(
        rpc_url_1024.to_string(),
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
    );

    let mut peer_ctxs: HashMap<u64, OutboundPeerCtx> = HashMap::new();
    for peer in peers {
        let evm_provider = if peer.kind == ChainKind::Evm {
            Some(Provider::<Http>::try_from(&peer.rpc_url).context("create peer EVM provider")?)
        } else {
            None
        };
        let evm_address = if peer.kind == ChainKind::Evm {
            Some(bytes32_to_evm_address(&peer.peer_contract)?)
        } else {
            None
        };
        let svm_rpc = if peer.kind == ChainKind::Svm {
            Some(Arc::new(RpcClient::new_with_commitment(
                peer.rpc_url.clone(),
                solana_sdk::commitment_config::CommitmentConfig::finalized(),
            )))
        } else {
            None
        };

        info!(
            chain_id = peer.chain_id,
            kind = %peer.kind,
            direction = "outbound",
            "Registered outbound peer"
        );

        peer_ctxs.insert(peer.chain_id, OutboundPeerCtx {
            peer: peer.clone(),
            evm_provider,
            evm_address,
            svm_rpc,
            pending_retry: Vec::new(),
        });
    }

    const OUTBOUND_CHECKPOINT_ID: u64 = 0;

    let mut last_sig = match load_svm_checkpoint(&checkpoints_dir, Direction::Outbound, OUTBOUND_CHECKPOINT_ID)? {
        Some(cp) => {
            Some(Signature::from_str(&cp.last_signature).context("parse saved outbound signature")?)
        }
        None => {
            info!(max_sigs = SVM_MAX_SIGS, "No outbound checkpoint, scanning recent signatures");
            None
        }
    };

    info!(peer_count = peer_ctxs.len(), "Starting unified outbound poller");

    loop {
        let mut handles: Vec<EvtHandle> = Vec::new();

        // Spawn retries for all peers in parallel
        for ctx in peer_ctxs.values_mut() {
            let retries: Vec<_> = ctx.pending_retry.drain(..).collect();
            for event in retries {
                handles.push(ctx.spawn_confirm(evm_wallet, usdc_mint, token_program_id, kp_bytes, event));
            }
        }

        // Poll 1024 chain once, spawn new events in parallel
        match svm::poller::poll_svm_events(&rpc_1024, program_id, last_sig.as_ref(), SVM_SIG_BATCH, SVM_MAX_SIGS) {
            Ok((events, newest_sig)) => {
                for event in events {
                    if let Some(ctx) = peer_ctxs.get(&event.target_chain_id) {
                        handles.push(ctx.spawn_confirm(evm_wallet, usdc_mint, token_program_id, kp_bytes, event));
                    } else {
                        warn!(
                            target_chain_id = event.target_chain_id,
                            nonce = event.nonce,
                            "No peer registered for target chain, skipping"
                        );
                    }
                }

                if let Some(sig) = newest_sig {
                    last_sig = Some(sig);
                    let cp = SvmCheckpoint {
                        last_signature: sig.to_string(),
                    };
                    if let Err(e) = save_svm_checkpoint(&checkpoints_dir, Direction::Outbound, OUTBOUND_CHECKPOINT_ID, &cp) {
                        warn!("Failed to save outbound checkpoint: {e}");
                    }
                }
            }
            Err(e) => {
                warn!("Outbound poll error: {e}");
            }
        }

        // Collect failures back into per-peer retry queues
        for event in collect_failures(handles).await {
            if let Some(ctx) = peer_ctxs.get_mut(&event.target_chain_id) {
                ctx.pending_retry.push(event);
            }
        }

        sleep(POLL_INTERVAL).await;
    }
}

/// Process a single event by submitting confirm_event to an SVM target chain.
/// Returns true if the event was handled (confirmed or already processed), false if it should be retried.
async fn process_event_for_svm(
    rpc: &RpcClient,
    program_id: &Pubkey,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    relayer_keypair: &solana_sdk::signature::Keypair,
    event: &StakeEventData,
    peer_chain_id: u64,
    direction: Direction,
) -> bool {
    let delay_ms = rand::thread_rng().gen_range(0..1000);
    sleep(Duration::from_millis(delay_ms)).await;

    match svm::submitter::check_nonce_processed(rpc, program_id, event.source_chain_id, event.nonce) {
        Ok(true) => {
            info!(
                nonce = event.nonce,
                peer_chain_id,
                direction = %direction,
                "Nonce already processed on SVM, skipping"
            );
            return true;
        }
        Ok(false) => {}
        Err(e) => {
            warn!(
                nonce = event.nonce,
                peer_chain_id,
                direction = %direction,
                "Failed to check SVM nonce status: {e}"
            );
            return false;
        }
    }

    match svm::submitter::submit_confirm_event(rpc, program_id, relayer_keypair, usdc_mint, token_program_id, event) {
        Ok(sig) => {
            info!(
                nonce = event.nonce,
                peer_chain_id,
                direction = %direction,
                tx = %sig,
                "Successfully submitted SVM confirm_event"
            );
            true
        }
        Err(e) => {
            warn!(
                nonce = event.nonce,
                peer_chain_id,
                direction = %direction,
                "Failed to submit SVM confirm_event: {e}"
            );
            false
        }
    }
}

/// Process a single event by submitting confirmEvent to an EVM target chain.
/// Returns true if the event was handled (confirmed or already processed), false if it should be retried.
async fn process_event_for_evm(
    evm_wallet: &LocalWallet,
    provider: &Provider<Http>,
    contract_address: Address,
    chain_id: u64,
    event: &StakeEventData,
    direction: Direction,
) -> bool {
    let delay_ms = rand::thread_rng().gen_range(0..1000);
    sleep(Duration::from_millis(delay_ms)).await;

    match evm::submitter::check_nonce_processed(provider, contract_address, event.nonce).await {
        Ok(true) => {
            info!(
                nonce = event.nonce,
                chain_id,
                direction = %direction,
                "Nonce already processed on EVM, skipping"
            );
            return true;
        }
        Ok(false) => {}
        Err(e) => {
            warn!(
                nonce = event.nonce,
                chain_id,
                direction = %direction,
                "Failed to check EVM nonce status: {e}"
            );
            return false;
        }
    }

    match evm::submitter::submit_confirm_event(evm_wallet, provider, contract_address, chain_id, event).await {
        Ok(tx_hash) => {
            info!(
                nonce = event.nonce,
                chain_id,
                direction = %direction,
                tx_hash = ?tx_hash,
                "Successfully submitted EVM confirmEvent"
            );
            true
        }
        Err(e) => {
            warn!(
                nonce = event.nonce,
                chain_id,
                direction = %direction,
                "Failed to submit EVM confirmEvent: {e}"
            );
            false
        }
    }
}

/// Convert a bytes32 peer_contract to an EVM Address.
fn bytes32_to_evm_address(bytes32: &[u8; 32]) -> Result<Address> {
    Ok(Address::from_slice(&bytes32[12..]))
}
