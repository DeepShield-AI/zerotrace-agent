//! 采集器（Source）— Real / Noop / Mock 三层实现。
//!
//! ADR-001 要求：每个组件提供三种实现：
//! - **Real**：生产实现
//! - **Noop**：空实现（配置 disable 时使用）
//! - **Mock**：测试桩（`#[cfg(any(test, feature = "test-utils"))]`）
//!
//! 测试可以直接用 Mock 替换真实 Source，无需经过 World 或 YAML。

use crate::signals::DemoMetric;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::mpsc;
use zerotrace_core::{
    error::Result,
    signal::{Batch, BatchMetadata},
};
use zerotrace_runtime::pipeline::Source;

// ═══════════════════════════════════════════════════════════════════════
// Real: PeriodicDemoSource — 定时产生 DemoMetric 批次
// ═══════════════════════════════════════════════════════════════════════

pub struct PeriodicDemoSource {
    pub name: &'static str,
    pub interval_ms: u64,
    pub batch_size: usize,
    pub metric_names: Vec<String>,
    counter: Arc<AtomicU64>,
}

impl PeriodicDemoSource {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            interval_ms: 1000,
            batch_size: 3,
            metric_names: vec!["demo.value".into()],
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_interval(mut self, ms: u64) -> Self {
        self.interval_ms = ms;
        self
    }
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }
    pub fn with_metric_names(mut self, names: Vec<String>) -> Self {
        self.metric_names = names;
        self
    }
    pub fn counter(&self) -> Arc<AtomicU64> {
        self.counter.clone()
    }
}

impl Source for PeriodicDemoSource {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn run(&mut self, sink: mpsc::Sender<Batch>) -> Result<()> {
        let interval = tokio::time::Duration::from_millis(self.interval_ms);
        let mut tick = tokio::time::interval(interval);
        tick.tick().await; // 跳过首次立即触发

        let metadata = Arc::new(BatchMetadata::new(self.name));

        loop {
            tick.tick().await;

            let mut batch = Batch::new(metadata.clone());
            let mut val = self.counter.fetch_add(1, Ordering::Relaxed);

            for i in 0..self.batch_size {
                let name = &self.metric_names[i % self.metric_names.len()];
                let value = (val as f64) * 100.0 + (i as f64) * 10.0;
                let now_ns = system_time_ns();

                batch.push(
                    DemoMetric::gauge(name.as_str(), value, now_ns)
                        .with_label("source", self.name)
                        .with_label("batch_index", i.to_string()),
                );
                val = val.wrapping_add(1);
            }

            if sink.send(batch).await.is_err() {
                tracing::info!("[{}] downstream closed, stopping", self.name);
                break;
            }
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Noop: NoopDemoSource — 空实现
// ═══════════════════════════════════════════════════════════════════════

/// 当 YAML 中 `source.enabled: false` 时使用此实现。
/// 所有方法均为空操作，不产生任何数据。
pub struct NoopDemoSource {
    pub name: &'static str,
}

impl NoopDemoSource {
    pub fn new(name: &'static str) -> Self {
        Self { name }
    }
}

impl Source for NoopDemoSource {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn run(&mut self, _sink: mpsc::Sender<Batch>) -> Result<()> {
        // Noop: 永远等待，直到被 shutdown
        std::future::pending::<()>().await;
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Mock: MockDemoSource — 测试桩
// ═══════════════════════════════════════════════════════════════════════

/// 测试用：发射预置的 Batch 列表，然后立即结束。
///
/// 不依赖任何外部 I/O，不需要 tokio runtime 的特殊配置。
/// 始终编译（不在 #[cfg(test)] 后）以便集成测试使用。
pub struct MockDemoSource {
    pub name: &'static str,
    pub batches: Vec<Batch>,
}

impl MockDemoSource {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            batches: vec![],
        }
    }

    pub fn with_batches(mut self, batches: Vec<Batch>) -> Self {
        self.batches = batches;
        self
    }

    /// 快捷构造：从 DemoMetric 列表创建一个 Batch。
    pub fn from_metrics(name: &'static str, metrics: Vec<DemoMetric>) -> Self {
        let meta = Arc::new(BatchMetadata::new(name));
        let mut batch = Batch::new(meta);
        for m in metrics {
            batch.push(m);
        }
        Self {
            name,
            batches: vec![batch],
        }
    }
}

impl Source for MockDemoSource {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn run(&mut self, sink: mpsc::Sender<Batch>) -> Result<()> {
        for batch in self.batches.drain(..) {
            if sink.send(batch).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

// ── 辅助 ──────────────────────────────────────────────────────────────

fn system_time_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

// ═══════════════════════════════════════════════════════════════════════
// 单元测试 — 直接调 trait 方法，不经过 Pipeline
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use zerotrace_core::signal::BatchMetadata;

    /// Level 1 单元测试：直接测试 Source trait 方法。
    /// 不需要 World、不需要 Bundle、不需要 Pipeline。
    #[tokio::test]
    async fn test_mock_source_emits_preset_batches() {
        let meta = Arc::new(BatchMetadata::new("test"));
        let mut batch1 = Batch::new(meta.clone());
        batch1.push(DemoMetric::gauge("cpu", 80.0, 1000));
        let mut batch2 = Batch::new(meta);
        batch2.push(DemoMetric::gauge("mem", 50.0, 2000));

        let mut source = MockDemoSource::new("mock").with_batches(vec![batch1, batch2]);

        let (tx, mut rx) = mpsc::channel(4);
        source.run(tx).await.unwrap();

        let mut received = vec![];
        while let Ok(b) = rx.try_recv() {
            received.push(b);
        }
        assert_eq!(received.len(), 2);
        assert_eq!(received[0].len(), 1);
        assert_eq!(received[1].len(), 1);
    }

    #[tokio::test]
    async fn test_mock_source_from_metrics_shortcut() {
        let metrics = vec![
            DemoMetric::gauge("a", 1.0, 0),
            DemoMetric::gauge("b", 2.0, 0),
            DemoMetric::gauge("c", 3.0, 0),
        ];
        let mut source = MockDemoSource::from_metrics("test", metrics);

        let (tx, mut rx) = mpsc::channel(4);
        source.run(tx).await.unwrap();

        let batch = rx.try_recv().unwrap();
        assert_eq!(batch.len(), 3);
        let names: Vec<&str> =
            batch.filter::<DemoMetric>().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_real_source_has_defaults() {
        let s = PeriodicDemoSource::new("test");
        assert_eq!(s.name, "test");
        assert_eq!(s.interval_ms, 1000);
        assert_eq!(s.batch_size, 3);
    }
}
