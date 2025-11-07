use anyhow::{Result, Context};
use serde::Deserialize;
use tracing::info;

#[derive(Deserialize)]
struct VAAResponse {
    vaa_hex: String,
    emitter_chain: u16,
    emitter_address: String,
    sequence: u64,
    signatures_count: usize,
}

pub async fn fetch_vaa(
    guardian_url: &str,
    chain: u16,
    emitter: &str,
    sequence: u64,
    output_file: Option<&str>,
) -> Result<()> {
    info!("🔍 Fetching VAA from Guardian API...");
    info!("   URL: {}", guardian_url);
    info!("   Chain: {}", chain);
    info!("   Emitter: {}", emitter);
    info!("   Sequence: {}", sequence);
    
    // Clean emitter address
    let emitter_clean = emitter.trim_start_matches("0x");
    
    // Build URL
    let url = format!(
        "{}/v1/signed_vaa/{}/{}/{}",
        guardian_url.trim_end_matches('/'),
        chain,
        emitter_clean,
        sequence
    );
    
    info!("   Request: {}", url);
    
    // Fetch VAA
    let response = reqwest::get(&url)
        .await
        .context("Failed to fetch VAA from Guardian")?;
    
    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Guardian returned {}: {}", status, error_text);
    }
    
    let vaa_data: VAAResponse = response
        .json()
        .await
        .context("Failed to parse VAA response")?;
    
    info!("✅ VAA retrieved successfully!");
    info!("   Signatures: {}", vaa_data.signatures_count);
    info!("   VAA length: {} bytes", (vaa_data.vaa_hex.len() - 2) / 2);
    
    // Decode hex
    let vaa_bytes = hex::decode(vaa_data.vaa_hex.trim_start_matches("0x"))
        .context("Invalid VAA hex")?;
    
    // Save to file or print
    if let Some(path) = output_file {
        std::fs::write(path, &vaa_bytes)
            .context("Failed to write VAA file")?;
        info!("💾 VAA saved to: {}", path);
    } else {
        println!("\nVAA (hex): 0x{}", hex::encode(&vaa_bytes));
        println!("VAA (base64): {}", base64_encode(&vaa_bytes));
    }
    
    Ok(())
}

fn base64_encode(data: &[u8]) -> String {
    use base64::{engine::general_purpose, Engine as _};
    general_purpose::STANDARD.encode(data)
}

