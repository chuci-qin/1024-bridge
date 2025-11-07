use anyhow::{Result, Context};
use clap::{Parser, Subcommand};
use tracing::{info, error};

mod commands;

#[derive(Parser)]
#[command(name = "bridge-cli")]
#[command(about = "Cross-chain bridge CLI tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    
    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch VAA from Guardian API
    FetchVaa {
        /// Guardian API URL
        #[arg(long)]
        guardian_url: String,
        
        /// Source chain ID
        #[arg(long)]
        chain: u16,
        
        /// Emitter address (hex)
        #[arg(long)]
        emitter: String,
        
        /// Sequence number
        #[arg(long)]
        sequence: u64,
        
        /// Output file
        #[arg(short, long)]
        output: Option<String>,
    },
    
    /// Submit VAA to destination chain
    SubmitVaa {
        /// Chain type: evm or solana
        #[arg(long)]
        chain: String,
        
        /// RPC URL
        #[arg(long)]
        rpc_url: String,
        
        /// VAA file or hex string
        #[arg(long)]
        vaa: String,
        
        /// Private key (EVM) or keypair file (Solana)
        #[arg(long)]
        key: String,
        
        /// Contract address (EVM only)
        #[arg(long)]
        contract: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(&cli.log_level)
        .init();
    
    match cli.command {
        Commands::FetchVaa {
            guardian_url,
            chain,
            emitter,
            sequence,
            output,
        } => {
            commands::fetch_vaa(&guardian_url, chain, &emitter, sequence, output.as_deref()).await
        }
        Commands::SubmitVaa {
            chain,
            rpc_url,
            vaa,
            key,
            contract,
        } => {
            commands::submit_vaa(&chain, &rpc_url, &vaa, &key, contract.as_deref()).await
        }
    }
}

