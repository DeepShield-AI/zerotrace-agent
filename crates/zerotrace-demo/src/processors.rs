//! 处理器（Processor）— Real / Noop / Mock 三层实现。
//!
//! Processor 的 `process(&mut self, batch: &mut Batch)` 是纯 async 函数，
//! 不依赖 World、不依赖 tokio task、不依赖 channel。
//! 这使它成为**最容易测试的组件** — 直接构造 Batch，调 process，断言结果。

use crate::signals::DemoMetric;
use zerotrace_core::{error::Result, signal::Batch};
use zerotrace_runtime::pipeline::Processor;

// ═══════════════════════════════════════════════════════════════════════
// Real: EnrichProcessor — 给 DemoMetric 加标签
// ═══════════════════════════════════════════════════════════════════════

#[derive(Clone)]
pub struct EnrichProcessor {
    pub name: &'static str,
    pub key: String,
    pub val: String,
}

impl EnrichProcessor {
    pub fn new(name: &'static str, key: impl Into<String>, val: impl Into<String>) -> Self {
        Self {
            name,
            key: key.into(),
            val: val.into(),
        }
    }
}

impl Processor for EnrichProcessor {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn process(&mut self, batch: &mut Batch) -> Result<()> {
        let items = batch.drain();
        for item in items {
            if let Some(metric) = item.downcast::<DemoMetric>() {
                let mut enriched = metric.clone();
                enriched.labels.push((self.key.clone(), self.val.clone()));
                batch.push(enriched);
            } else {
                batch.push_any(item); // 非 DemoMetric 透传
            }
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Real: ThresholdFilter — 按阈值过滤 DemoMetric
// ═══════════════════════════════════════════════════════════════════════

pub struct ThresholdFilter {
    pub name: &'static str,
    pub min_value: f64,
    pub filtered_count: u64,
}

impl ThresholdFilter {
    pub fn new(name: &'static str, min_value: f64) -> Self {
        Self {
            name,
            min_value,
            filtered_count: 0,
        }
    }
}

impl Processor for ThresholdFilter {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn process(&mut self, batch: &mut Batch) -> Result<()> {
        let items = batch.drain();
        let mut filtered = 0u64;
        for item in items {
            let keep = if let Some(m) = item.downcast::<DemoMetric>() {
                m.value >= self.min_value
            } else {
                true // 非 DemoMetric 总是通过
            };
            if keep {
                batch.push_any(item);
            } else {
                filtered += 1;
            }
        }
        self.filtered_count += filtered;
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Noop: NoopProcessor — 空实现
// ═══════════════════════════════════════════════════════════════════════

pub struct NoopProcessor {
    pub name: &'static str,
}

impl NoopProcessor {
    pub fn new(name: &'static str) -> Self {
        Self { name }
    }
}

impl Processor for NoopProcessor {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn process(&mut self, _batch: &mut Batch) -> Result<()> {
        Ok(()) // 什么也不做
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Mock: 测试用处理器
// ═══════════════════════════════════════════════════════════════════════

/// 记录每次 process 调用的参数，供测试断言。
pub struct SpyProcessor {
    pub name: &'static str,
    pub call_count: u64,
    pub item_counts: Vec<usize>,
}

impl SpyProcessor {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            call_count: 0,
            item_counts: vec![],
        }
    }
}

impl Processor for SpyProcessor {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn process(&mut self, batch: &mut Batch) -> Result<()> {
        self.call_count += 1;
        self.item_counts.push(batch.len());
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 单元测试 — 直接调 Processor::process，零依赖
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use zerotrace_core::signal::BatchMetadata;

    fn make_demo_batch() -> Batch {
        let meta = Arc::new(BatchMetadata::new("test"));
        let mut batch = Batch::new(meta);
        batch.push(DemoMetric::gauge("cpu", 85.0, 1000).with_label("host", "n1"));
        batch.push(DemoMetric::gauge("mem", 42.0, 1000));
        batch.push(DemoMetric::gauge("disk", -1.0, 1000)); // 会被阈值过滤
        batch
    }

    // ── Level 1: EnrichProcessor 单元测试 ──────────────────────────

    #[tokio::test]
    async fn test_enrich_adds_label_to_all_metrics() {
        let mut proc = EnrichProcessor::new("enrich", "env", "staging");
        let mut batch = make_demo_batch();
        let original_len = batch.len();

        proc.process(&mut batch).await.unwrap();

        assert_eq!(batch.len(), original_len);
        for item in &batch.items {
            let m = item.downcast::<DemoMetric>().unwrap();
            assert!(
                m.labels.iter().any(|(k, v)| k == "env" && v == "staging"),
                "expected label env=staging on metric {}",
                m.name
            );
        }
    }

    #[tokio::test]
    async fn test_enrich_is_idempotent_when_called_twice() {
        let mut proc = EnrichProcessor::new("enrich", "region", "us-east");
        let mut batch = make_demo_batch();

        proc.process(&mut batch).await.unwrap();
        proc.process(&mut batch).await.unwrap(); // 第二次

        // 标签应该出现两次（因为每次都 drain → 重建）
        for item in &batch.items {
            let m = item.downcast::<DemoMetric>().unwrap();
            let count = m.labels.iter().filter(|(k, _)| k == "region").count();
            assert_eq!(count, 2, "label should appear twice after double enrich");
        }
    }

    // ── Level 1: ThresholdFilter 单元测试 ──────────────────────────

    #[tokio::test]
    async fn test_threshold_filter_drops_below_min() {
        let mut proc = ThresholdFilter::new("filter", 0.0);
        let mut batch = make_demo_batch();
        let original_len = batch.len();

        proc.process(&mut batch).await.unwrap();

        // disk 值 -1.0 应被过滤
        assert_eq!(batch.len(), original_len - 1);
        assert!(proc.filtered_count == 1);
        for item in &batch.items {
            let m = item.downcast::<DemoMetric>().unwrap();
            assert!(
                m.value >= 0.0,
                "metric {} has value {} < 0",
                m.name,
                m.value
            );
        }
    }

    #[tokio::test]
    async fn test_threshold_filter_all_pass_when_min_is_low() {
        let mut proc = ThresholdFilter::new("filter", -999.0);
        let mut batch = make_demo_batch();
        let original_len = batch.len();

        proc.process(&mut batch).await.unwrap();
        assert_eq!(batch.len(), original_len); // 全部通过
        assert_eq!(proc.filtered_count, 0);
    }

    #[tokio::test]
    async fn test_threshold_filter_all_dropped_when_min_is_high() {
        let mut proc = ThresholdFilter::new("filter", 999.0);
        let mut batch = make_demo_batch();
        proc.process(&mut batch).await.unwrap();
        assert!(batch.is_empty()); // 全部被过滤
    }

    // ── Level 1: NoopProcessor ────────────────────────────────────

    #[tokio::test]
    async fn test_noop_preserves_batch() {
        let mut proc = NoopProcessor::new("noop");
        let mut batch = make_demo_batch();
        let original_len = batch.len();

        proc.process(&mut batch).await.unwrap();
        assert_eq!(batch.len(), original_len);
    }

    // ── Level 1: SpyProcessor ──────────────────────────────────────

    #[tokio::test]
    async fn test_spy_records_invocations() {
        let mut spy = SpyProcessor::new("spy");
        let mut batch = make_demo_batch();

        spy.process(&mut batch).await.unwrap();
        assert_eq!(spy.call_count, 1);
        assert_eq!(spy.item_counts, vec![3]);

        spy.process(&mut batch).await.unwrap();
        assert_eq!(spy.call_count, 2);
        assert_eq!(spy.item_counts, vec![3, 3]);
    }

    // ── 链式测试：多个 processor 串联后的业务效果 ────────────────

    #[tokio::test]
    async fn test_enrich_then_filter_chain_behavior() {
        // Arrange: enrich → filter 链
        let mut enrich = EnrichProcessor::new("enrich", "env", "prod");
        let mut filter = ThresholdFilter::new("filter", 0.0);
        let mut batch = make_demo_batch();

        // Act: 先 enrich 后 filter
        enrich.process(&mut batch).await.unwrap();
        filter.process(&mut batch).await.unwrap();

        // Assert: disk 被过滤，剩余的 cpu/mem 有 env=prod 标签
        assert_eq!(batch.len(), 2);
        for item in &batch.items {
            let m = item.downcast::<DemoMetric>().unwrap();
            assert!(m.value >= 0.0);
            assert!(m.labels.iter().any(|(k, v)| k == "env" && v == "prod"));
        }
        assert_eq!(filter.filtered_count, 1);
    }
}
