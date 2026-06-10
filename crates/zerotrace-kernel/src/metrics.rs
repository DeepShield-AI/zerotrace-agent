// Internal observability metrics for the DI framework itself.
//
// These counters track the health of the kernel at runtime — channel
// backpressure, bundle load time, config dispatch latency, etc.  Exposed
// through the debug socket (future: Prometheus text format).
//
// Design:
//   - All counters use `AtomicU64` for zero-contention updates on hot paths.
//   - A `KernelMetrics` struct is registered in the World at startup.
//   - Components read metrics via `Res<KernelMetrics>`.

use std::sync::atomic::{AtomicU64, Ordering};

/// Aggregated kernel metrics.  Thread-safe; all fields may be updated
/// concurrently from multiple tokio tasks.
#[derive(Debug)]
pub struct KernelMetrics {
    /// P50 world_get latency in nanoseconds (approximate, best-effort).
    pub world_get_count: AtomicU64,
    pub world_get_total_ns: AtomicU64,

    /// P99 world_get latency tracking (simple: count calls exceeding 1µs).
    pub world_get_slow_count: AtomicU64,

    /// Current aggregate length of all pipeline channels.
    pub pipeline_channel_len: AtomicU64,

    /// Total bundles loaded since startup.
    pub bundle_load_total: AtomicU64,

    /// Cumulative bundle load duration in milliseconds.
    pub bundle_load_total_ms: AtomicU64,

    /// Total lifecycle start_all calls (should be 1).
    pub lifecycle_startup_total: AtomicU64,

    /// Cumulative lifecycle startup duration in milliseconds.
    pub lifecycle_startup_total_ms: AtomicU64,

    /// Total config change dispatches.
    pub config_dispatch_total: AtomicU64,

    /// Cumulative config dispatch duration in milliseconds.
    pub config_dispatch_total_ms: AtomicU64,
}

impl KernelMetrics {
    pub fn new() -> Self {
        Self {
            world_get_count: AtomicU64::new(0),
            world_get_total_ns: AtomicU64::new(0),
            world_get_slow_count: AtomicU64::new(0),
            pipeline_channel_len: AtomicU64::new(0),
            bundle_load_total: AtomicU64::new(0),
            bundle_load_total_ms: AtomicU64::new(0),
            lifecycle_startup_total: AtomicU64::new(0),
            lifecycle_startup_total_ms: AtomicU64::new(0),
            config_dispatch_total: AtomicU64::new(0),
            config_dispatch_total_ms: AtomicU64::new(0),
        }
    }

    // ── Helpers ──────────────────────────────────────────────────

    /// Record a `world_get` call with its latency in nanoseconds.
    pub fn record_world_get(&self, latency_ns: u64) {
        self.world_get_count.fetch_add(1, Ordering::Relaxed);
        self.world_get_total_ns
            .fetch_add(latency_ns, Ordering::Relaxed);
        if latency_ns > 1000 {
            self.world_get_slow_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Approximate average world_get latency (0 if no calls).
    pub fn world_get_avg_ns(&self) -> u64 {
        let count = self.world_get_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0;
        }
        self.world_get_total_ns.load(Ordering::Relaxed) / count
    }

    /// Fraction of world_get calls that exceeded 1µs (0.0–1.0).
    pub fn world_get_slow_ratio(&self) -> f64 {
        let count = self.world_get_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        self.world_get_slow_count.load(Ordering::Relaxed) as f64 / count as f64
    }

    /// Average bundle load time in milliseconds.
    pub fn bundle_load_avg_ms(&self) -> u64 {
        let count = self.bundle_load_total.load(Ordering::Relaxed);
        if count == 0 {
            return 0;
        }
        self.bundle_load_total_ms.load(Ordering::Relaxed) / count
    }

    /// Average config dispatch time in milliseconds.
    pub fn config_dispatch_avg_ms(&self) -> u64 {
        let count = self.config_dispatch_total.load(Ordering::Relaxed);
        if count == 0 {
            return 0;
        }
        self.config_dispatch_total_ms.load(Ordering::Relaxed) / count
    }

    /// Snapshot all metrics as key-value pairs (for debug socket).
    pub fn snapshot(&self) -> Vec<(&'static str, u64)> {
        vec![
            (
                "world_get_count",
                self.world_get_count.load(Ordering::Relaxed),
            ),
            ("world_get_avg_ns", self.world_get_avg_ns()),
            (
                "world_get_slow_ratio_pct",
                (self.world_get_slow_ratio() * 100.0) as u64,
            ),
            (
                "pipeline_channel_len",
                self.pipeline_channel_len.load(Ordering::Relaxed),
            ),
            (
                "bundle_load_total",
                self.bundle_load_total.load(Ordering::Relaxed),
            ),
            ("bundle_load_avg_ms", self.bundle_load_avg_ms()),
            (
                "lifecycle_startup_total_ms",
                self.lifecycle_startup_total_ms.load(Ordering::Relaxed),
            ),
            (
                "config_dispatch_total",
                self.config_dispatch_total.load(Ordering::Relaxed),
            ),
            ("config_dispatch_avg_ms", self.config_dispatch_avg_ms()),
        ]
    }
}

impl Default for KernelMetrics {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_avg() {
        let m = KernelMetrics::new();
        m.record_world_get(100);
        m.record_world_get(200);
        m.record_world_get(300);
        assert_eq!(m.world_get_avg_ns(), 200);
    }

    #[test]
    fn test_slow_ratio() {
        let m = KernelMetrics::new();
        // 1 fast, 1 slow
        m.record_world_get(500); // fast
        m.record_world_get(1500); // slow (> 1000ns)
        let ratio = m.world_get_slow_ratio();
        assert!((ratio - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_empty_metrics_zero() {
        let m = KernelMetrics::new();
        assert_eq!(m.world_get_avg_ns(), 0);
        assert_eq!(m.world_get_slow_ratio(), 0.0);
        assert_eq!(m.bundle_load_avg_ms(), 0);
    }

    #[test]
    fn test_snapshot_has_all_keys() {
        let m = KernelMetrics::new();
        let snap = m.snapshot();
        assert!(snap.len() >= 9);
        // Verify key fields exist
        let keys: Vec<&str> = snap.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"world_get_count"));
        assert!(keys.contains(&"pipeline_channel_len"));
        assert!(keys.contains(&"config_dispatch_total"));
    }
}
