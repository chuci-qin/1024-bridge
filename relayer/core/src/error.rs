use thiserror::Error;

#[derive(Error, Debug)]
pub enum BridgeError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("Signing error: {0}")]
    Signing(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Queue error: {0}")]
    Queue(String),

    #[error("Chain error: {0}")]
    Chain(String),

    #[error("Retry exhausted for nonce {nonce} after {retries} attempts")]
    RetryExhausted { nonce: u64, retries: u32 },

    #[error("Checkpoint error: {0}")]
    Checkpoint(String),

    #[error("Invalid event data: {0}")]
    InvalidEvent(String),
}

impl From<std::io::Error> for BridgeError {
    fn from(e: std::io::Error) -> Self {
        BridgeError::Config(e.to_string())
    }
}

impl From<serde_json::Error> for BridgeError {
    fn from(e: serde_json::Error) -> Self {
        BridgeError::Serialization(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = BridgeError::Config("missing file".into());
        assert_eq!(err.to_string(), "Configuration error: missing file");
    }

    #[test]
    fn test_retry_exhausted_display() {
        let err = BridgeError::RetryExhausted {
            nonce: 42,
            retries: 5,
        };
        assert_eq!(
            err.to_string(),
            "Retry exhausted for nonce 42 after 5 attempts"
        );
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let bridge_err: BridgeError = io_err.into();
        assert!(matches!(bridge_err, BridgeError::Config(_)));
    }

    #[test]
    fn test_from_serde_error() {
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let bridge_err: BridgeError = serde_err.into();
        assert!(matches!(bridge_err, BridgeError::Serialization(_)));
    }
}
