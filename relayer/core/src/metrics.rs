use std::sync::atomic::{AtomicU64, Ordering};

/// Simple atomic counters for bridge relayer observability.
pub struct Metrics {
    pub events_received: AtomicU64,
    pub events_submitted: AtomicU64,
    pub events_failed: AtomicU64,
    pub events_dead_lettered: AtomicU64,
    pub last_processed_nonce: AtomicU64,
    pub last_checkpoint_block: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            events_received: AtomicU64::new(0),
            events_submitted: AtomicU64::new(0),
            events_failed: AtomicU64::new(0),
            events_dead_lettered: AtomicU64::new(0),
            last_processed_nonce: AtomicU64::new(0),
            last_checkpoint_block: AtomicU64::new(0),
        }
    }

    /// Snapshot all counters into a JSON value suitable for health endpoints.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "events_received": self.events_received.load(Ordering::Relaxed),
            "events_submitted": self.events_submitted.load(Ordering::Relaxed),
            "events_failed": self.events_failed.load(Ordering::Relaxed),
            "events_dead_lettered": self.events_dead_lettered.load(Ordering::Relaxed),
            "last_processed_nonce": self.last_processed_nonce.load(Ordering::Relaxed),
            "last_checkpoint_block": self.last_checkpoint_block.load(Ordering::Relaxed),
        })
    }

    pub fn inc_received(&self) {
        self.events_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_submitted(&self) {
        self.events_submitted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_failed(&self) {
        self.events_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_dead_lettered(&self) {
        self.events_dead_lettered.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_last_nonce(&self, nonce: u64) {
        self.last_processed_nonce.store(nonce, Ordering::Relaxed);
    }

    pub fn set_last_block(&self, block: u64) {
        self.last_checkpoint_block.store(block, Ordering::Relaxed);
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_metrics_are_zero() {
        let m = Metrics::new();
        assert_eq!(m.events_received.load(Ordering::Relaxed), 0);
        assert_eq!(m.events_submitted.load(Ordering::Relaxed), 0);
        assert_eq!(m.events_failed.load(Ordering::Relaxed), 0);
        assert_eq!(m.events_dead_lettered.load(Ordering::Relaxed), 0);
        assert_eq!(m.last_processed_nonce.load(Ordering::Relaxed), 0);
        assert_eq!(m.last_checkpoint_block.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_increment_counters() {
        let m = Metrics::new();
        m.inc_received();
        m.inc_received();
        m.inc_submitted();
        m.inc_failed();
        m.inc_dead_lettered();

        assert_eq!(m.events_received.load(Ordering::Relaxed), 2);
        assert_eq!(m.events_submitted.load(Ordering::Relaxed), 1);
        assert_eq!(m.events_failed.load(Ordering::Relaxed), 1);
        assert_eq!(m.events_dead_lettered.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_set_last_nonce_and_block() {
        let m = Metrics::new();
        m.set_last_nonce(42);
        m.set_last_block(12345);
        assert_eq!(m.last_processed_nonce.load(Ordering::Relaxed), 42);
        assert_eq!(m.last_checkpoint_block.load(Ordering::Relaxed), 12345);
    }

    #[test]
    fn test_to_json() {
        let m = Metrics::new();
        m.inc_received();
        m.inc_received();
        m.inc_submitted();
        m.set_last_nonce(99);
        m.set_last_block(5000);

        let json = m.to_json();
        assert_eq!(json["events_received"], 2);
        assert_eq!(json["events_submitted"], 1);
        assert_eq!(json["events_failed"], 0);
        assert_eq!(json["events_dead_lettered"], 0);
        assert_eq!(json["last_processed_nonce"], 99);
        assert_eq!(json["last_checkpoint_block"], 5000);
    }

    #[test]
    fn test_default_equals_new() {
        let m = Metrics::default();
        assert_eq!(m.events_received.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_concurrent_increments() {
        use std::sync::Arc;
        use std::thread;

        let m = Arc::new(Metrics::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let m = Arc::clone(&m);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    m.inc_received();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.events_received.load(Ordering::Relaxed), 1000);
    }
}
