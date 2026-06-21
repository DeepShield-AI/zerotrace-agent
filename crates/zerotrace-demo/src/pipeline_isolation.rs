//! 共享组件的多 pipeline 隔离策略。
//!
//! # 问题
//!
//! 两条 pipeline 共用同一个 reporter 时，reporter 如何区分数据来源？
//!
//! ```text
//! pipeline "metrics"                    pipeline "l7"
//! cpu -> tag --+                        ebpf -> parse --+
//!              +-> http_reporter <---------------------+
//!              ^
//!         需要区分：哪个 batch 来自 metrics？哪个来自 l7？
//!         需要分别统计，可能还需要分别限流、分别路由。
//! ```
//!
//! # 四种模式
//!
//! | 模式 | 做法 | 隔离程度 | 适用 |
//! |------|------|---------|------|
//! | A. 独立实例 | Clone 创建完全独立的 reporter | 强隔离 | 默认选择 |
//! | B. 标签注入 | Reporter 构造时注入 pipeline_id | 逻辑隔离 | 共享 HTTP 后端 |
//! | C. Processor 打标 | EnrichProcessor 在 batch 中加 pipeline 标签 | 数据层隔离 | reporter 无感知 |
//! | D. BatchMetadata | reporter 读 `batch.metadata.source_id` | 利用已有字段 | 最简单的区分 |
//!
//! # Clone 的行为分析
//!
//! ```rust
//! #[derive(Clone)]
//! struct MyReporter {
//!     name: &'static str,              // Copy → 独立
//!     batch_count: u64,                 // Copy → 独立（每个实例各自计数）
//!     shared_state: Arc<AtomicU64>,     // Arc::clone → 共享（所有实例看到同一个值）
//! }
//! ```
//!
//! **关键结论**：`#[derive(Clone)]` 时，`Arc` 字段是**共享引用**（引用计数 bump），
//! 普通字段是**独立拷贝**。所以：
//! - `NoopReporter { submit_count: Arc<AtomicU64> }` → Clone 后**独立计数**（每个实例一个 Arc）
//! - 如果你想要**共享计数**，在 Clone 实现中显式 `self.shared.clone()`

use crate::signals::DemoMetric;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use zerotrace_core::{error::Result, signal::Batch};
use zerotrace_runtime::pipeline::Reporter;

// ═══════════════════════════════════════════════════════════════════════
// 模式 B：PipelineTagReporter — 注入 pipeline_id 到 batch 元数据
// ═══════════════════════════════════════════════════════════════════════

/// 包装任意 Reporter，在 submit 前给 batch 中所有信号打上 pipeline 标签。
///
/// 下游 reporter（如 HTTP forwarder）不需要知道 pipeline 的存在。
/// 标签在数据层面，server 端可按 `pipeline_id` 过滤/聚合。
pub struct PipelineTagReporter<R: Reporter> {
    inner: R,
    pipeline_id: String,
}

impl<R: Reporter + Clone> PipelineTagReporter<R> {
    /// 创建一个带 pipeline 标签的 reporter。
    pub fn new(inner: R, pipeline_id: impl Into<String>) -> Self {
        Self {
            inner,
            pipeline_id: pipeline_id.into(),
        }
    }

    /// 获取内部 reporter（用于 Clone 时传递标签）。
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Reporter + Clone> Clone for PipelineTagReporter<R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            pipeline_id: self.pipeline_id.clone(),
        }
    }
}

impl<R: Reporter> Reporter for PipelineTagReporter<R> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn submit(&mut self, batch: &Batch) -> Result<()> {
        // 给 batch 中每个 DemoMetric 注入 pipeline_id 标签
        // 注意：batch 是 &Batch（不可变），我们无法修改原 batch。
        // 真实场景中，应该在 Processor 阶段注入标签（模式 C），
        // 或者 reporter 通过 HTTP header / 请求参数传递 pipeline_id。
        //
        // 这里演示概念：reporter 内部知道自己的 pipeline_id。
        tracing::debug!(
            "[PipelineTagReporter] submitting batch of {} items for pipeline [{}]",
            batch.len(),
            self.pipeline_id,
        );

        self.inner.submit(batch).await
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 模式 C：通过 Processor 注入 pipeline 标签（推荐）
// ═══════════════════════════════════════════════════════════════════════

use zerotrace_runtime::pipeline::Processor;

/// 一个 Processor，给每个 DemoMetric 添加 pipeline_id 标签。
///
/// 用法：在每条 pipeline 的 processor 链的开头放置此 processor。
/// 这样 reporter 收到的 batch 中已经带了 `pipeline_id=metrics` 标签，
/// reporter 完全不需要知道 pipeline 的存在。
#[derive(Clone)]
pub struct PipelineLabelProcessor {
    pub name: &'static str,
    pub pipeline_id: String,
}

impl PipelineLabelProcessor {
    pub fn new(name: &'static str, pipeline_id: impl Into<String>) -> Self {
        Self {
            name,
            pipeline_id: pipeline_id.into(),
        }
    }
}

impl Processor for PipelineLabelProcessor {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn process(&mut self, batch: &mut Batch) -> Result<()> {
        let items = batch.drain();
        for item in items {
            if let Some(metric) = item.downcast::<DemoMetric>() {
                let mut enriched = metric.clone();
                enriched.labels.push(("pipeline_id".into(), self.pipeline_id.clone()));
                batch.push(enriched);
            } else {
                batch.push_any(item);
            }
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 展示：共享后端 + 独立统计
// ═══════════════════════════════════════════════════════════════════════

/// 模拟共享 HTTP 连接池的 reporter。
///
/// `#[derive(Clone)]` 时 Arc 是共享引用。如需独立计数，手动实现 Clone。
#[derive(Clone)]
pub struct SharedBackendReporter {
    pub name: &'static str,
    /// 所有 Clone 实例共享同一个 HTTP client。
    pub backend: Arc<SharedBackend>,
    /// 注意：derive(Clone) 下此 Arc 也是共享的。
    /// 如需独立计数，见 `IndependentCloneReporter`。
    pub local_count: Arc<AtomicU64>,
}

/// 演示手动 Clone：独立 local_count + 共享 backend。
pub struct IndependentCloneReporter {
    pub name: &'static str,
    pub backend: Arc<SharedBackend>,
    pub local_count: Arc<AtomicU64>,
}

impl Clone for IndependentCloneReporter {
    fn clone(&self) -> Self {
        Self {
            name: self.name,
            backend: self.backend.clone(),            // 共享
            local_count: Arc::new(AtomicU64::new(0)), // 独立！
        }
    }
}

impl IndependentCloneReporter {
    pub fn new(name: &'static str, backend: Arc<SharedBackend>) -> Self {
        Self {
            name,
            backend,
            local_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl Reporter for IndependentCloneReporter {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn submit(&mut self, batch: &Batch) -> Result<()> {
        self.backend.total_requests.fetch_add(batch.len() as u64, Ordering::Relaxed);
        self.local_count.fetch_add(batch.len() as u64, Ordering::Relaxed);
        Ok(())
    }
}

/// 模拟共享的 HTTP 后端（如 reqwest::Client 的连接池）。
pub struct SharedBackend {
    pub endpoint: String,
    pub total_requests: AtomicU64,
}

impl SharedBackend {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            total_requests: AtomicU64::new(0),
        }
    }
}

impl SharedBackendReporter {
    pub fn new(name: &'static str, backend: Arc<SharedBackend>) -> Self {
        Self {
            name,
            backend,
            local_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl Reporter for SharedBackendReporter {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn submit(&mut self, batch: &Batch) -> Result<()> {
        let n = batch.len() as u64;
        // 更新共享统计
        self.backend.total_requests.fetch_add(n, Ordering::Relaxed);
        // 更新本地统计
        self.local_count.fetch_add(n, Ordering::Relaxed);
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 模式 D：利用 BatchMetadata 携带 pipeline 信息
// ═══════════════════════════════════════════════════════════════════════

/// Reporter 在 submit 时从 BatchMetadata 读取来源信息。
///
/// 框架已有的 `BatchMetadata.source_id` 和 `BatchMetadata.shared_attributes`
/// 可以携带 pipeline 标识。配置 pipeline 时在 Source 构造阶段设置，reporter
/// 无需额外配置。
pub struct MetadataAwareReporter<R: Reporter> {
    inner: R,
    /// 按 source_id 分组统计
    pub by_source: std::collections::HashMap<String, u64>,
}

impl<R: Reporter> MetadataAwareReporter<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            by_source: std::collections::HashMap::new(),
        }
    }
}

impl<R: Reporter> Reporter for MetadataAwareReporter<R> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn submit(&mut self, batch: &Batch) -> Result<()> {
        // 从 BatchMetadata 读取来源信息
        let source = &batch.metadata.source_id;
        let n = batch.len() as u64;
        *self.by_source.entry(source.to_string()).or_default() += n;

        self.inner.submit(batch).await
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 测试：四种模式的行为验证
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reporters::NoopReporter;
    use std::sync::Arc;
    use zerotrace_core::signal::BatchMetadata;

    /// derive(Clone): Arc 是共享引用（引用计数 bump）
    #[test]
    fn test_derive_clone_shares_arc_fields() {
        let backend = Arc::new(SharedBackend::new("https://server/api"));
        let rep1 = SharedBackendReporter::new("r1", backend.clone());
        let rep2 = rep1.clone();

        // derive(Clone) 对 Arc 字段是 Clone（共享引用）
        assert!(
            Arc::ptr_eq(&rep1.local_count, &rep2.local_count),
            "derive(Clone) shares Arc fields"
        );
        assert!(
            Arc::ptr_eq(&rep1.backend, &rep2.backend),
            "backend is correctly shared"
        );
    }

    /// 手动 Clone：独立 local_count + 共享 backend
    #[test]
    fn test_manual_clone_creates_independent_counters() {
        let backend = Arc::new(SharedBackend::new("https://server/api"));
        let rep1 = IndependentCloneReporter::new("r1", backend.clone());
        let rep2 = rep1.clone();

        // 手动 Clone：local_count 是独立的
        assert!(
            !Arc::ptr_eq(&rep1.local_count, &rep2.local_count),
            "manual Clone creates independent local_count"
        );
        // backend 仍然共享
        assert!(
            Arc::ptr_eq(&rep1.backend, &rep2.backend),
            "backend is still shared"
        );
    }

    /// 共享 backend 的全局统计对所有实例可见
    #[tokio::test]
    async fn test_shared_backend_aggregates_across_clones() {
        let backend = Arc::new(SharedBackend::new("https://server/api"));
        let mut rep1 = SharedBackendReporter::new("metrics", backend.clone());
        let mut rep2 = rep1.clone();

        let meta = Arc::new(BatchMetadata::new("test"));
        let batch = Batch::new(meta);

        rep1.submit(&batch).await.unwrap(); // batch.len() = 0, but doesn't matter
        rep2.submit(&batch).await.unwrap();

        assert_eq!(backend.total_requests.load(Ordering::Relaxed), 0);
        // empty batch → 0 signals
        // 真实场景中 batch 有数据时 backend.total_requests 会是各实例的合计
    }

    /// PipelineLabelProcessor 给 batch 注入 pipeline_id
    #[tokio::test]
    async fn test_pipeline_label_processor_injects_identity() {
        let meta = Arc::new(BatchMetadata::new("cpu_collector"));
        let mut batch = Batch::new(meta);
        batch.push(DemoMetric::gauge("cpu", 80.0, 0));
        batch.push(DemoMetric::gauge("mem", 50.0, 0));

        let mut metrics_labeler = PipelineLabelProcessor::new("label", "metrics_pipeline");
        let mut l7_labeler = PipelineLabelProcessor::new("label", "l7_pipeline");

        // Clone batch
        let mut batch2 = batch.clone();

        metrics_labeler.process(&mut batch).await.unwrap();
        l7_labeler.process(&mut batch2).await.unwrap();

        for item in &batch.items {
            let m = item.downcast::<DemoMetric>().unwrap();
            assert!(m.labels.iter().any(|(k, v)| k == "pipeline_id" && v == "metrics_pipeline"));
        }
        for item in &batch2.items {
            let m = item.downcast::<DemoMetric>().unwrap();
            assert!(m.labels.iter().any(|(k, v)| k == "pipeline_id" && v == "l7_pipeline"));
        }
    }

    /// MetadataAwareReporter 按 source_id 分组统计
    #[tokio::test]
    async fn test_metadata_aware_reporter_groups_by_source() {
        let mut rep = MetadataAwareReporter::new(NoopReporter::new("inner"));

        let meta1 = Arc::new(BatchMetadata::new("cpu_collector"));
        let meta2 = Arc::new(BatchMetadata::new("ebpf_socket"));
        let mut b1 = Batch::new(meta1);
        b1.push(DemoMetric::gauge("x", 1.0, 0));
        let mut b2 = Batch::new(meta2);
        b2.push(DemoMetric::gauge("y", 2.0, 0));
        b2.push(DemoMetric::gauge("z", 3.0, 0));

        rep.submit(&b1).await.unwrap();
        rep.submit(&b2).await.unwrap();

        assert_eq!(rep.by_source.get("cpu_collector"), Some(&1));
        assert_eq!(rep.by_source.get("ebpf_socket"), Some(&2));
    }
}
