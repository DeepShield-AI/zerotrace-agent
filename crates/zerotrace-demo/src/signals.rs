//! Custom signal types for the demo.
//!
//! Demonstrates `#[derive(SignalType)]` — the proc macro that
//! auto-implements `zerotrace_core::signal::SignalType`.

use zerotrace_kernel_derive::SignalType;

/// A demo metric signal produced by the collector.
///
/// `#[signal(kind = "demo.metric")]` maps to `SignalKind("demo.metric")`
/// in the generated impl.  Well-known strings like `"metric"`, `"trace"`,
/// `"log"`, `"profile"`, `"event"` map to their respective built-in
/// constants; everything else is passed directly to the `SignalKind`
/// struct constructor (which is an open identifier — no enum exhaustiveness).
#[derive(Debug, Clone, SignalType)]
#[signal(kind = "demo.metric")]
pub struct DemoMetric {
    /// The metric name (e.g. "cpu.utilization").
    pub name: String,
    /// The metric value.
    pub value: f64,
    /// Nanosecond timestamp when the metric was collected.
    pub timestamp_ns: i64,
    /// Arbitrary key-value labels attached to the metric.
    pub labels: Vec<(String, String)>,
}

impl DemoMetric {
    /// Create a new gauge-style metric.
    pub fn gauge(name: impl Into<String>, value: f64, timestamp_ns: i64) -> Self {
        Self {
            name: name.into(),
            value,
            timestamp_ns,
            labels: Vec::new(),
        }
    }

    /// Attach a label to this metric (builder pattern).
    pub fn with_label(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.labels.push((key.into(), val.into()));
        self
    }
}
