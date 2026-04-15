use std::path::Path;

use anyhow::{Context, Result};
use ethers::signers::{LocalWallet, Signer};
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer as SolSigner;
use tracing::warn;

/// Loaded relayer key material.
pub struct Keys {
    pub svm_keypair: Keypair,
    pub evm_wallet: LocalWallet,
}

#[derive(serde::Serialize)]
struct Addresses {
    svm_pubkey: String,
    evm_address: String,
}

impl Keys {
    /// Load (or auto-generate) keys from `keys_dir`, write `addresses.json`,
    /// and log public addresses at WARN level.
    pub fn load_or_generate(keys_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(keys_dir)?;

        let svm_path = keys_dir.join("svm_keypair.json");
        let evm_path = keys_dir.join("evm_private_key.txt");

        let svm_keypair = load_or_generate_svm(&svm_path)?;
        let evm_wallet = load_or_generate_evm(&evm_path)?;

        let svm_pubkey = svm_keypair.pubkey().to_string();
        let evm_address = format!("{:?}", evm_wallet.address());

        let addresses = Addresses {
            svm_pubkey: svm_pubkey.clone(),
            evm_address: evm_address.clone(),
        };
        let addr_path = keys_dir.join("addresses.json");
        let json = serde_json::to_string_pretty(&addresses)?;
        std::fs::write(&addr_path, json)
            .with_context(|| format!("Failed to write {}", addr_path.display()))?;

        warn!(svm_pubkey = %svm_pubkey, "SVM public key (add to bridge relayers list)");
        warn!(evm_address = %evm_address, "EVM address (add to bridge relayers list)");

        Ok(Keys {
            svm_keypair,
            evm_wallet,
        })
    }
}

fn load_or_generate_svm(path: &Path) -> Result<Keypair> {
    if path.exists() {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let bytes: Vec<u8> = serde_json::from_str(&data)
            .with_context(|| format!("Invalid Solana keypair JSON in {}", path.display()))?;
        let kp = Keypair::try_from(bytes.as_slice())
            .with_context(|| "Invalid Solana keypair bytes")?;
        Ok(kp)
    } else {
        let kp = Keypair::new();
        let bytes = kp.to_bytes().to_vec();
        let json = serde_json::to_string(&bytes)?;
        std::fs::write(path, &json)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        warn!("SVM keypair auto-generated at {}", path.display());
        Ok(kp)
    }
}

fn load_or_generate_evm(path: &Path) -> Result<LocalWallet> {
    if path.exists() {
        let hex_str = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let hex_str = hex_str.trim().trim_start_matches("0x");
        let wallet: LocalWallet = hex_str
            .parse()
            .with_context(|| "Invalid EVM private key hex")?;
        Ok(wallet)
    } else {
        let mut rng = rand::thread_rng();
        let wallet = LocalWallet::new(&mut rng);
        let key_bytes = wallet.signer().to_bytes();
        let hex_str = hex::encode(key_bytes);
        std::fs::write(path, &hex_str)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        warn!("EVM key auto-generated at {}", path.display());
        Ok(wallet)
    }
}
