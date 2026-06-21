//! 上报器（Reporter）— Real / Noop 实现 + 测试支持。
//!
//! Reporter 的 `submit(&mut self, batch: &Batch)` 只读 Batch，
//! 不修改数据 — 纯输出操作。

use crate::signals::DemoMetric;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use zerotrace_core::{error::Result, signal::Batch};
use zerotrace_runtime::pipeline::Reporter;

// ═══════════════════════════════════════════════════════════════════════
// Real: ConsoleReporter — 输出到 stdout
// ═══════════════════════════════════════════════════════════════════════

#[derive(Clone)]
pub struct ConsoleReporter {
    pub name: &'static str,
    pub format: ConsoleFormat,
    pub log_interval_batches: usize,
    batch_count: u64,
    signal_count: Arc<AtomicU64>,
    total_signals: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleFormat {
    Summary,
    Json,
}

impl ConsoleReporter {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            format: ConsoleFormat::Summary,
            log_interval_batches: 5,
            batch_count: 0,
            signal_count: Arc::new(AtomicU64::new(0)),
            total_signals: Arc::new(AtomicU64::new(0)),
        }
    }
    pub fn with_format(mut self, fmt: ConsoleFormat) -> Self {
        self.format = fmt;
        self
    }
    pub fn with_log_interval(mut self, n: usize) -> Self {
        self.log_interval_batches = n;
        self
    }
    pub fn signal_count(&self) -> Arc<AtomicU64> {
        self.signal_count.clone()
    }
    #[allow(dead_code)]
    pub fn total_signals(&self) -> Arc<AtomicU64> {
        self.total_signals.clone()
    }
}

impl Reporter for ConsoleReporter {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn submit(&mut self, batch: &Batch) -> Result<()> {
        self.batch_count += 1;
        let n = batch.len() as u64;
        self.signal_count.fetch_add(n, Ordering::Relaxed);
        self.total_signals.fetch_add(n, Ordering::Relaxed);

        match self.format {
            ConsoleFormat::Summary => self.print_summary(batch),
            ConsoleFormat::Json => self.print_json(batch),
        }

        if self.batch_count % self.log_interval_batches as u64 == 0 {
            println!(
                "[{}] 📊 {} batches, {} signals (total: {})",
                self.name,
                self.batch_count,
                self.signal_count.load(Ordering::Relaxed),
                self.total_signals.load(Ordering::Relaxed),
            );
        }
        Ok(())
    }
}

impl ConsoleReporter {
    fn print_summary(&self, batch: &Batch) {
        let metrics: Vec<&DemoMetric> = batch.filter::<DemoMetric>();
        if metrics.is_empty() {
            println!("[{}] batch #{}: empty", self.name, self.batch_count);
            return;
        }
        let mut by_name: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        for m in &metrics {
            by_name.entry(&m.name).or_default().push(m.value);
        }
        let parts: Vec<String> = by_name
            .iter()
            .map(|(name, vals)| {
                format!("{name}={:.1}", vals.iter().sum::<f64>() / vals.len() as f64)
            })
            .collect();
        println!(
            "[{}] batch #{}: {}",
            self.name,
            self.batch_count,
            parts.join(", ")
        );
    }

    fn print_json(&self, batch: &Batch) {
        let metrics: Vec<&DemoMetric> = batch.filter::<DemoMetric>();
        let entries: Vec<String> = metrics
            .iter()
            .map(|m| {
                let labels: Vec<String> =
                    m.labels.iter().map(|(k, v)| format!("\"{k}\":\"{v}\"")).collect();
                format!(
                    "{{\"name\":\"{}\",\"value\":{:.2},\"ts\":{},\"labels\":{{{}}}}}",
                    m.name,
                    m.value,
                    m.timestamp_ns,
                    labels.join(",")
                )
            })
            .collect();
        println!(
            "[{}] batch #{}: [{}]",
            self.name,
            self.batch_count,
            entries.join(", ")
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Noop: NoopReporter — 空实现
// ═══════════════════════════════════════════════════════════════════════

#[derive(Clone)]
pub struct NoopReporter {
    pub name: &'static str,
    pub submit_count: Arc<AtomicU64>,
}

impl NoopReporter {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            submit_count: Arc::new(AtomicU64::new(0)),
        }
    }
    pub fn submit_count(&self) -> Arc<AtomicU64> {
        self.submit_count.clone()
    }
}

impl Reporter for NoopReporter {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn submit(&mut self, batch: &Batch) -> Result<()> {
        self.submit_count.fetch_add(batch.len() as u64, Ordering::Relaxed);
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 单元测试
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::DemoMetric;
    use std::sync::Arc;
    use zerotrace_core::signal::BatchMetadata;

    fn make_batch() -> Batch {
        let meta = Arc::new(BatchMetadata::new("test"));
        let mut batch = Batch::new(meta);
        batch.push(DemoMetric::gauge("cpu", 80.0, 1000));
        batch.push(DemoMetric::gauge("mem", 50.0, 1000));
        batch
    }

    #[tokio::test]
    async fn test_console_reporter_increments_counters() {
        let mut rep = ConsoleReporter::new("test").with_log_interval(1000);
        rep.submit(&make_batch()).await.unwrap();
        assert_eq!(rep.batch_count, 1);
        assert_eq!(rep.signal_count.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_noop_reporter_counts_signals() {
        let mut rep = NoopReporter::new("noop");
        rep.submit(&make_batch()).await.unwrap();
        rep.submit(&make_batch()).await.unwrap();
        assert_eq!(rep.submit_count.load(Ordering::Relaxed), 4);
    }

    #[tokio::test]
    async fn test_noop_reporter_empty_batch() {
        let mut rep = NoopReporter::new("noop");
        let meta = Arc::new(BatchMetadata::new("test"));
        let batch = Batch::new(meta);
        rep.submit(&batch).await.unwrap();
        assert_eq!(rep.submit_count.load(Ordering::Relaxed), 0);
    }
}
