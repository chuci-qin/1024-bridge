use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::error::BridgeError;
use crate::types::{BridgeEvent, CompactStakeEventData};

// ---------------------------------------------------------------------------
// Ed25519
// ---------------------------------------------------------------------------

/// Sign `message` with an Ed25519 keypair, returning the 64-byte signature.
pub fn sign_ed25519(message: &[u8], keypair: &ed25519_dalek::SigningKey) -> Vec<u8> {
    use ed25519_dalek::Signer;
    keypair.sign(message).to_bytes().to_vec()
}

/// Verify an Ed25519 signature against a 32-byte public key.
pub fn verify_ed25519(message: &[u8], signature: &[u8], pubkey: &[u8; 32]) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(vk) = VerifyingKey::from_bytes(pubkey) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(signature) else {
        return false;
    };
    vk.verify(message, &sig).is_ok()
}

/// Construct a [`ed25519_dalek::SigningKey`] from raw bytes, zeroizing the
/// source buffer after construction (REL-M8).
pub fn load_ed25519_keypair(secret_bytes: &mut [u8; 32]) -> ed25519_dalek::SigningKey {
    let key = ed25519_dalek::SigningKey::from_bytes(secret_bytes);
    secret_bytes.zeroize();
    key
}

// ---------------------------------------------------------------------------
// ECDSA / EIP-191  (EVM)
// ---------------------------------------------------------------------------

/// Sign a 32-byte hash using EIP-191 personal-sign via an ethers `LocalWallet`.
///
/// The wallet applies the `\x19Ethereum Signed Message:\n32` prefix internally
/// before signing.  Returns 65 bytes (r ‖ s ‖ v).
pub async fn sign_ecdsa_eip191(
    message_hash: &[u8; 32],
    wallet: &ethers::signers::LocalWallet,
) -> Result<Vec<u8>, BridgeError> {
    use ethers::signers::Signer;
    let sig = wallet
        .sign_message(message_hash.as_ref())
        .await
        .map_err(|e| BridgeError::Signing(format!("EIP-191 signing failed: {e}")))?;
    Ok(sig.to_vec())
}

// ---------------------------------------------------------------------------
// Hashing helpers
// ---------------------------------------------------------------------------

/// Produce a deterministic SHA-256 hash of the event data serialized as a
/// canonical JSON string.  The field order and quoting match the on-chain
/// verifier expectations.
pub fn hash_event_data_json(event: &BridgeEvent) -> [u8; 32] {
    let json = format!(
        r#"{{"sourceContract":"{}","targetContract":"{}","chainId":"{}","blockHeight":"{}","amount":"{}","sender":"{}","receiverAddress":"{}","nonce":"{}"}}"#,
        event.source_contract,
        event.target_contract,
        event.source_chain_id,
        event.block_height,
        event.amount,
        event.sender,
        event.receiver_address,
        event.nonce,
    );
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    hasher.finalize().into()
}

// ---------------------------------------------------------------------------
// Borsh helpers (SVM)
// ---------------------------------------------------------------------------

/// Borsh-serialize a [`CompactStakeEventData`] for Ed25519 signing on SVM.
pub fn serialize_event_borsh(event: &CompactStakeEventData) -> Vec<u8> {
    borsh::to_vec(event).expect("Borsh serialization of CompactStakeEventData must not fail")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bridge_event() -> BridgeEvent {
        BridgeEvent {
            source_contract: "aa".repeat(32),
            target_contract: "bb".repeat(32),
            source_chain_id: 1,
            target_chain_id: 2,
            block_height: 100,
            amount: 500,
            sender: "01".repeat(32),
            receiver_address: "cc".repeat(32),
            nonce: 7,
        }
    }

    // -- Ed25519 ---------------------------------------------------------

    #[test]
    fn test_ed25519_sign_and_verify() {
        let secret = [0x42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret);
        let pubkey = signing_key.verifying_key().to_bytes();

        let message = b"bridge1024 test message";
        let sig = sign_ed25519(message, &signing_key);
        assert_eq!(sig.len(), 64);
        assert!(verify_ed25519(message, &sig, &pubkey));
    }

    #[test]
    fn test_ed25519_verify_wrong_message() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[0x42u8; 32]);
        let pubkey = signing_key.verifying_key().to_bytes();

        let sig = sign_ed25519(b"correct", &signing_key);
        assert!(!verify_ed25519(b"wrong", &sig, &pubkey));
    }

    #[test]
    fn test_ed25519_verify_bad_pubkey() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[0x42u8; 32]);
        let sig = sign_ed25519(b"msg", &signing_key);
        assert!(!verify_ed25519(b"msg", &sig, &[0xFF; 32]));
    }

    #[test]
    fn test_ed25519_verify_bad_signature() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[0x42u8; 32]);
        let pubkey = signing_key.verifying_key().to_bytes();
        assert!(!verify_ed25519(b"msg", &[0u8; 64], &pubkey));
    }

    #[test]
    fn test_ed25519_verify_short_signature() {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[0x42u8; 32]);
        let pubkey = signing_key.verifying_key().to_bytes();
        assert!(!verify_ed25519(b"msg", &[0u8; 10], &pubkey));
    }

    // -- Key loading with zeroize ----------------------------------------

    #[test]
    fn test_load_ed25519_keypair_zeroizes_input() {
        let mut secret = [0x42u8; 32];
        let key = load_ed25519_keypair(&mut secret);
        assert_eq!(secret, [0u8; 32], "secret bytes must be zeroized");
        let sig = sign_ed25519(b"test", &key);
        assert_eq!(sig.len(), 64);
    }

    // -- JSON hashing ----------------------------------------------------

    #[test]
    fn test_hash_event_data_json_deterministic() {
        let event = sample_bridge_event();
        let h1 = hash_event_data_json(&event);
        let h2 = hash_event_data_json(&event);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_event_data_json_changes_with_nonce() {
        let mut e1 = sample_bridge_event();
        let mut e2 = sample_bridge_event();
        e2.nonce = 999;
        assert_ne!(hash_event_data_json(&e1), hash_event_data_json(&e2));

        e1.nonce = 999;
        assert_eq!(hash_event_data_json(&e1), hash_event_data_json(&e2));
    }

    #[test]
    fn test_hash_event_data_json_format() {
        let event = BridgeEvent {
            source_contract: "ab".repeat(32),
            target_contract: "cd".repeat(32),
            source_chain_id: 42,
            target_chain_id: 1024,
            block_height: 999,
            amount: 100,
            sender: "00".repeat(32),
            receiver_address: "ef".repeat(32),
            nonce: 1,
        };
        let hash = hash_event_data_json(&event);
        assert_eq!(hash.len(), 32);

        let expected_json = format!(
            r#"{{"sourceContract":"{}","targetContract":"{}","chainId":"42","blockHeight":"999","amount":"100","sender":"{}","receiverAddress":"{}","nonce":"1"}}"#,
            "ab".repeat(32),
            "cd".repeat(32),
            "00".repeat(32),
            "ef".repeat(32),
        );
        let mut hasher = Sha256::new();
        hasher.update(expected_json.as_bytes());
        let expected_hash: [u8; 32] = hasher.finalize().into();
        assert_eq!(hash, expected_hash);
    }

    // -- Borsh serialization ---------------------------------------------

    #[test]
    fn test_serialize_event_borsh_roundtrip() {
        use borsh::BorshDeserialize;
        let compact = CompactStakeEventData {
            nonce: 10,
            amount: 2000,
            block_height: 555,
            sender: [0x11; 32],
            receiver_address: [0x22; 32],
        };
        let bytes = serialize_event_borsh(&compact);
        let decoded = CompactStakeEventData::try_from_slice(&bytes).unwrap();
        assert_eq!(compact, decoded);
    }

    #[test]
    fn test_serialize_event_borsh_deterministic() {
        let compact = CompactStakeEventData {
            nonce: 1,
            amount: 2,
            block_height: 3,
            sender: [0xAA; 32],
            receiver_address: [0xBB; 32],
        };
        assert_eq!(
            serialize_event_borsh(&compact),
            serialize_event_borsh(&compact)
        );
    }

    // -- ECDSA EIP-191 ---------------------------------------------------

    #[tokio::test]
    async fn test_sign_ecdsa_eip191() {
        use ethers::signers::LocalWallet;
        let wallet: LocalWallet =
            "0x4c0883a69102937d6231471b5dbb6204fe512961708279f21ee73662bfcef7ab"
                .parse()
                .unwrap();
        let hash = [0xAA; 32];
        let sig = sign_ecdsa_eip191(&hash, &wallet).await.unwrap();
        assert_eq!(sig.len(), 65, "ECDSA+v signature must be 65 bytes");
    }

    #[tokio::test]
    async fn test_sign_ecdsa_eip191_deterministic() {
        use ethers::signers::LocalWallet;
        let wallet: LocalWallet =
            "0x4c0883a69102937d6231471b5dbb6204fe512961708279f21ee73662bfcef7ab"
                .parse()
                .unwrap();
        let hash = [0xBB; 32];
        let s1 = sign_ecdsa_eip191(&hash, &wallet).await.unwrap();
        let s2 = sign_ecdsa_eip191(&hash, &wallet).await.unwrap();
        assert_eq!(s1, s2);
    }
}
