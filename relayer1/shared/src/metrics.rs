use prometheus::{
    register_counter_vec, register_gauge_vec, register_histogram_vec, CounterVec, GaugeVec,
    HistogramVec, TextEncoder, Encoder,
};
use std::sync::OnceLock;

static EVENTS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
static LATENCY_SECONDS: OnceLock<HistogramVec> = OnceLock::new();
static QUEUE_SIZE: OnceLock<GaugeVec> = OnceLock::new();
static BALANCE: OnceLock<GaugeVec> = OnceLock::new();

/// 初始化 Prometheus 指标
pub fn init_metrics() {
    EVENTS_TOTAL.get_or_init(|| {
        register_counter_vec!(
            "relayer_events_total",
            "Total number of events processed",
            &["service", "status"]
        )
        .unwrap()
    });

    LATENCY_SECONDS.get_or_init(|| {
        register_histogram_vec!(
            "relayer_latency_seconds",
            "Event processing latency in seconds",
            &["service"]
        )
        .unwrap()
    });

    QUEUE_SIZE.get_or_init(|| {
        register_gauge_vec!(
            "relayer_queue_size",
            "Current queue size",
            &["service", "status"]
        )
        .unwrap()
    });

    BALANCE.get_or_init(|| {
        register_gauge_vec!(
            "relayer_balance",
            "Relayer account balance",
            &["service", "chain"]
        )
        .unwrap()
    });
}

/// 记录事件处理
pub fn record_event(service: &str, status: &str) {
    if let Some(counter) = EVENTS_TOTAL.get() {
        counter.with_label_values(&[service, status]).inc();
    }
}

/// 记录延迟
pub fn record_latency(service: &str, duration_secs: f64) {
    if let Some(histogram) = LATENCY_SECONDS.get() {
        histogram.with_label_values(&[service]).observe(duration_secs);
    }
}

/// 记录队列大小
pub fn record_queue_size(service: &str, status: &str, size: i64) {
    if let Some(gauge) = QUEUE_SIZE.get() {
        gauge.with_label_values(&[service, status]).set(size as f64);
    }
}

/// 记录余额
pub fn record_balance(service: &str, chain: &str, balance: f64) {
    if let Some(gauge) = BALANCE.get() {
        gauge.with_label_values(&[service, chain]).set(balance);
    }
}

/// 导出指标 (Prometheus 格式)
pub fn export_metrics() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_init() {
        init_metrics();
        record_event("s2e", "success");
        record_latency("s2e", 1.5);
        record_queue_size("s2e", "pending", 10);
        record_balance("s2e", "svm", 10.5);
        
        let metrics = export_metrics();
        assert!(metrics.contains("relayer_events_total"));
        assert!(metrics.contains("relayer_latency_seconds"));
        assert!(metrics.contains("relayer_queue_size"));
        assert!(metrics.contains("relayer_balance"));
    }
}

