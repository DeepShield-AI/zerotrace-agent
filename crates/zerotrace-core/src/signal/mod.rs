// ── Signal types for ZeroTrace agent  ────────────────────────────────
//
// Module structure:
//
//   kind.rs       — SignalKind (open identifier)
//   attributes.rs — AttrValue, AttrSet, AttributeSet, AttributeSetBuilder
//   resource.rs   — Resource (OTEL §Resource)
//   metric.rs     — MetricValue, MetricPoint (OTEL §Metric)
//   span.rs       — SpanKind, SpanStatus, Span (OTEL §Span)
//   log.rs        — Severity, LogRecord (OTEL §LogRecord)
//   profile.rs    — StackSample (profiling)
//   event.rs      — SystemEvent (structured events)
//   batch.rs      — BatchMetadata, TypedBatch<T>, Batch
//   mod.rs        — SignalType trait, AnySignal, re-exports, tests
//
// Design principles:
//   1. Resource — identity context shared via Arc<Resource>
//   2. AnySignal — universal type-erased container
//   3. TypedBatch<T> — compile-time homogeneous batch
//   4. MetricValue — discriminated Gauge/Counter/Histogram
//   5. SpanKind + SpanStatus — per OTEL spec
//   6. LogRecord carries trace_id + span_id — log/trace correlation
//   7. Attributes — typed Str/Int/Float/Bool via AttrValue
//   8. Cow<'static, str> — zero-alloc static path for high-frequency fields
//   9. SmallVec — inline up to 4 attributes on the stack
//  10. AttributeSet — Arc-backed, pre-sorted, pre-hashed, shared

use std::{
    any::{Any, TypeId},
    fmt,
    sync::Arc,
};

// ── Submodules ───────────────────────────────────────────────────────

pub mod attributes;
pub mod batch;
pub mod event;
pub mod kind;
pub mod log;
pub mod metric;
pub mod profile;
pub mod resource;
pub mod span;

// ── Re-exports ────────────────────────────────────────────────────────

pub use attributes::{AttrSet, AttrValue, AttributeSet, AttributeSetBuilder};
pub use batch::{Batch, BatchMetadata, TypedBatch};
pub use event::SystemEvent;
pub use kind::SignalKind;
pub use log::{LogRecord, Severity};
pub use metric::{MetricPoint, MetricValue};
pub use profile::StackSample;
pub use resource::Resource;
pub use span::{Span, SpanKind, SpanStatus};

// ═══════════════════════════════════════════════════════════════════════
// SignalType trait — the core abstraction
// ═══════════════════════════════════════════════════════════════════════

/// Trait for types that can be used as signal payloads.
///
/// Every signal type declares its kind and an estimated heap footprint.
/// Types implementing this trait can be:
///   - Wrapped in [`AnySignal`] for type-erased transport
///   - Collected in [`TypedBatch<T>`] for homogeneous batch processing
///   - Routed by [`SignalKind`] or [`TypeId`] without downcasting
///
/// ```
/// use zerotrace_core::signal::{SignalType, SignalKind};
///
/// #[derive(Debug, Clone)]
/// struct AnomalyDetected { score: f64 }
///
/// impl SignalType for AnomalyDetected {
///     fn signal_kind() -> SignalKind { SignalKind("ai.anomaly") }
/// }
/// ```
pub trait SignalType: fmt::Debug + Clone + Send + Sync + 'static {
    /// The canonical kind for routing and filtering.
    fn signal_kind() -> SignalKind;

    /// Approximate per-instance heap allocation in bytes.
    fn estimated_heap_bytes(&self) -> usize {
        0
    }

    /// Compile-time [`TypeId`] for zero-alloc channel routing.
    fn type_id() -> TypeId
    where
        Self: Sized,
    {
        TypeId::of::<Self>()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// AnySignal — universal type-erased signal container
// ═══════════════════════════════════════════════════════════════════════

/// A type-erased signal that can carry any [`SignalType`].
///
/// `AnySignal` stores the concrete `TypeId` and `SignalKind` *inline* so
/// routing decisions don't require a downcast virtual call.  The payload
/// lives behind an `Arc` — cloning an `AnySignal` is a ref-count bump.
#[derive(Debug, Clone)]
pub struct AnySignal {
    type_id: TypeId,
    kind: SignalKind,
    /// Cached estimate from [`SignalType::estimated_heap_bytes`] at
    /// construction time, so the outer [`estimated_heap_bytes`] can
    /// report a meaningful total without a virtual call.
    payload_heap: usize,
    payload: Arc<dyn Any + Send + Sync>,
}

impl AnySignal {
    pub fn new<T: SignalType>(value: T) -> Self {
        let payload_heap = value.estimated_heap_bytes();
        Self {
            type_id: TypeId::of::<T>(),
            kind: T::signal_kind(),
            payload_heap,
            payload: Arc::new(value),
        }
    }

    pub fn kind(&self) -> &SignalKind {
        &self.kind
    }
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }
    pub fn is<T: SignalType>(&self) -> bool {
        TypeId::of::<T>() == self.type_id
    }
    pub fn downcast<T: SignalType>(&self) -> Option<&T> {
        if self.is::<T>() {
            self.payload.downcast_ref::<T>()
        } else {
            None
        }
    }
    pub fn as_any(&self) -> &(dyn Any + Send + Sync) {
        &*self.payload
    }
    /// Estimated total heap footprint: struct overhead + fat-pointer +
    /// the inner type's own estimate (captured at construction time).
    pub fn estimated_heap_bytes(&self) -> usize {
        std::mem::size_of::<Self>() +
            std::mem::size_of::<Arc<dyn Any + Send + Sync>>() +
            self.payload_heap
    }
    /// Per-instance kind, which may differ from the static `SignalType::signal_kind()`.
    pub fn instance_kind(&self) -> &SignalKind {
        &self.kind
    }
}

// AnySignal is itself a SignalType, enabling TypedBatch<AnySignal>.
impl SignalType for AnySignal {
    fn signal_kind() -> SignalKind {
        SignalKind("any")
    }
    fn estimated_heap_bytes(&self) -> usize {
        self.estimated_heap_bytes()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    // ── Resource ──────────────────────────────────────────────────

    #[test]
    fn resource_builder_ergonomics() {
        let r = Resource::with_service("zerotrace")
            .with("host.name", "node-1")
            .with("deployment.environment", "production");
        assert_eq!(r.service_name, "zerotrace");
        assert_eq!(r.attributes.len(), 2);
    }

    // ── AttrValue ─────────────────────────────────────────────────

    #[test]
    fn attr_value_typed_constructors() {
        let s: AttrValue = "hello".into();
        let i: AttrValue = 42i64.into();
        let f: AttrValue = 3.14.into();
        let b: AttrValue = true.into();
        assert_eq!(s.as_str(), Some("hello"));
        assert_eq!(i.as_int(), Some(42));
        assert!((f.as_float().unwrap() - 3.14).abs() < 0.001);
        assert_eq!(b.as_bool(), Some(true));
    }

    #[test]
    fn attr_value_display() {
        assert_eq!(AttrValue::Int(200).to_string(), "200");
        assert_eq!(AttrValue::Bool(true).to_string(), "true");
        assert_eq!(AttrValue::Str(Cow::Borrowed("ok")).to_string(), "ok");
    }

    // ── AnySignal ─────────────────────────────────────────────────

    #[test]
    fn any_signal_roundtrip() {
        let m = MetricPoint::gauge("cpu.usage", 0.9, 5000).with_attr("host", "node-1");
        let any = AnySignal::new(m.clone());
        assert_eq!(any.kind(), &SignalKind::METRIC);
        assert!(any.is::<MetricPoint>());
        let recovered = any.downcast::<MetricPoint>().unwrap();
        assert_eq!(recovered.name, "cpu.usage");
        assert!((recovered.value.as_f64().unwrap() - 0.9).abs() < 0.001);
    }

    #[test]
    fn any_signal_wrong_type_returns_none() {
        let any = AnySignal::new(MetricPoint::gauge("x", 0.0, 0));
        assert!(any.downcast::<Span>().is_none());
    }

    #[test]
    fn any_signal_clone_is_arc_bump() {
        let any = AnySignal::new(MetricPoint::gauge("x", 1.0, 0));
        let any2 = any.clone();
        assert_eq!(any2.type_id(), any.type_id());
    }

    #[test]
    fn any_signal_with_custom_kind() {
        #[derive(Debug, Clone, PartialEq)]
        struct Anomaly {
            score: f64,
        }
        impl SignalType for Anomaly {
            fn signal_kind() -> SignalKind {
                SignalKind("ai.anomaly")
            }
        }
        let any = AnySignal::new(Anomaly { score: 0.95 });
        assert_eq!(any.kind(), &SignalKind("ai.anomaly"));
        assert!(any.downcast::<Anomaly>().is_some());
    }

    // ── MetricPoint ───────────────────────────────────────────────

    #[test]
    fn metric_gauge_builder() {
        let m = MetricPoint::gauge("cpu.temp", 72.5, 1000)
            .with_description("CPU temperature")
            .with_unit("Cel")
            .with_attr("host", "node-1")
            .with_attr("region", "us-east-1");
        assert_eq!(m.name, "cpu.temp");
        assert_eq!(m.value, MetricValue::Gauge(72.5));
        assert_eq!(m.description.as_deref(), Some("CPU temperature"));
        assert_eq!(m.unit, "Cel");
        assert_eq!(m.attributes.len(), 2);
    }

    #[test]
    fn metric_counter_builder() {
        let m = MetricPoint::counter("http.requests", 42.0, 2000).with_start_time(0);
        assert!(m.value.is_monotonic());
        assert_eq!(m.start_time_ns, Some(0));
    }

    #[test]
    fn metric_histogram() {
        let m = MetricPoint {
            name: Cow::Borrowed("http.latency"),
            description: None,
            unit: Cow::Borrowed("ms"),
            value: MetricValue::Histogram {
                bucket_counts: smallvec::smallvec![10, 5, 2],
                bucket_bounds: smallvec::smallvec![10.0, 50.0, 200.0],
                sum: 650.0,
                count: 17,
            },
            attributes: AttrSet::new(),
            timestamp_ns: 3000,
            start_time_ns: None,
        };
        assert!(m.value.as_f64().is_none());
        if let MetricValue::Histogram { sum, count, .. } = &m.value {
            assert!((*sum - 650.0).abs() < 0.01);
            assert_eq!(*count, 17);
        }
    }

    // ── Span ──────────────────────────────────────────────────────

    #[test]
    fn span_builder_with_status() {
        let s = Span::new([1; 16], [2; 8], "api-gateway", "POST /orders", 1000)
            .with_parent([3; 8])
            .with_kind(SpanKind::Server)
            .with_status(SpanStatus::Error {
                message: Cow::Borrowed("timeout"),
            })
            .with_attr("http.status_code", 504);
        assert_eq!(s.kind, SpanKind::Server);
        assert_eq!(
            s.status,
            Some(SpanStatus::Error {
                message: Cow::Borrowed("timeout")
            })
        );
        assert!(s.duration_secs() == 0.0);
    }

    #[test]
    fn span_trace_flags_default_sampled() {
        let s = Span::new([0; 16], [0; 8], "svc", "op", 0);
        assert_eq!(s.trace_flags & 1, 1);
    }

    // ── LogRecord ─────────────────────────────────────────────────

    #[test]
    fn log_record_with_trace_correlation() {
        let log = LogRecord::new(Severity::Error, "connection refused", 5000)
            .with_trace([1; 16], [2; 8])
            .with_attr("exception.type", "ConnectionError");
        assert_eq!(log.severity, Severity::Error);
        assert_eq!(log.trace_id, Some([1; 16]));
        assert_eq!(log.span_id, Some([2; 8]));
        assert_eq!(log.attributes[0].1.as_str(), Some("ConnectionError"));
    }

    #[test]
    fn severity_ordering() {
        assert!(Severity::Error > Severity::Warn);
        assert!(Severity::Fatal > Severity::Error);
        assert!(Severity::Debug > Severity::Trace);
    }

    // ── TypedBatch ────────────────────────────────────────────────

    #[test]
    fn typed_batch_homogeneous_by_construction() {
        let meta = Arc::new(BatchMetadata::new("test"));
        let mut batch = TypedBatch::<MetricPoint>::new(meta);
        batch.push(MetricPoint::gauge("cpu", 0.8, 1000));
        batch.push(MetricPoint::gauge("mem", 0.6, 1000));
        assert_eq!(batch.len(), 2);
        assert_eq!(TypedBatch::<MetricPoint>::kind(), SignalKind::METRIC);
    }

    #[test]
    fn typed_batch_drain() {
        let meta = Arc::new(BatchMetadata::new("test"));
        let items = vec![MetricPoint::gauge("x", 1.0, 0)];
        let mut batch = TypedBatch::from_vec(items, meta.clone());
        let drained = batch.drain();
        assert_eq!(drained.len(), 1);
        assert!(batch.is_empty());
    }

    // ── Batch ─────────────────────────────────────────────────────

    #[test]
    fn batch_mixed_types() {
        let meta = Arc::new(BatchMetadata::new("test"));
        let mut batch = Batch::new(meta);
        batch.push(MetricPoint::gauge("cpu", 0.5, 0));
        batch.push(Span::new([0; 16], [0; 8], "svc", "op", 0));
        batch.push(LogRecord::new(Severity::Info, "started", 0));
        assert_eq!(batch.len(), 3);
        assert!(!batch.all::<MetricPoint>());
        let metrics: Vec<&MetricPoint> = batch.filter::<MetricPoint>();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "cpu");
    }

    #[test]
    fn batch_into_typed_lossy() {
        let meta = Arc::new(BatchMetadata::new("test"));
        let mut batch = Batch::new(meta.clone());
        batch.push(MetricPoint::gauge("a", 1.0, 0));
        batch.push(MetricPoint::gauge("b", 2.0, 0));
        batch.push(Span::new([0; 16], [0; 8], "x", "y", 0));
        let typed = batch.into_typed_lossy::<MetricPoint>();
        assert_eq!(typed.len(), 2);
    }

    // ── BatchMetadata ─────────────────────────────────────────────

    #[test]
    fn batch_metadata_with_resource() {
        let resource = Arc::new(Resource::with_service("my-agent").with("host.name", "node-1"));
        let meta = BatchMetadata::new("collector").with_resource(resource);
        assert_eq!(meta.source_id, "collector");
        assert!(meta.resource.is_some());
        assert_eq!(meta.resource.as_ref().unwrap().service_name, "my-agent");
    }

    // ── Cow optimization ──────────────────────────────────────────

    #[test]
    fn static_string_is_borrowed() {
        let m = MetricPoint::gauge("cpu.usage", 0.8, 1000);
        assert!(matches!(&m.name, Cow::Borrowed("cpu.usage")));
    }

    // ── SignalKind open for extension ─────────────────────────────

    #[test]
    fn custom_signal_kind() {
        let kind = SignalKind("ai.anomaly");
        assert_eq!(kind.as_str(), "ai.anomaly");
    }

    // ── StackSample ───────────────────────────────────────────────

    #[test]
    fn stack_sample_with_thread() {
        let s = StackSample::new("nginx", 1234, 1000).with_thread(5678);
        assert_eq!(s.pid, 1234);
        assert_eq!(s.thread_id, Some(5678));
    }

    #[test]
    fn stack_sample_with_frames() {
        let s = StackSample::new("app", 1, 0).with_frames(&[0x1000, 0x2000, 0x3000]);
        assert_eq!(s.stack_frames.len(), 3);
    }

    // ── SystemEvent ───────────────────────────────────────────────

    #[test]
    fn system_event_builder() {
        let e = SystemEvent::new("kernel", "oom", "out of memory in cgroup", 1000);
        assert_eq!(e.domain, "kernel");
        assert_eq!(e.name, "oom");
    }

    #[test]
    fn system_event_with_attr() {
        let e = SystemEvent::new("kernel", "oom", "memory exhausted", 1000)
            .with_attr("cgroup", "/system.slice");
        assert_eq!(e.attributes.len(), 1);
    }

    // ═════════════════════════════════════════════════════════════
    // AttributeSet tests
    // ═════════════════════════════════════════════════════════════

    #[test]
    fn attribute_set_builder_empty() {
        let set = AttributeSetBuilder::new().build();
        assert!(set.is_empty());
        assert!(set.get("anything").is_none());
    }

    #[test]
    fn attribute_set_builder_with_pairs() {
        let set = AttributeSetBuilder::new()
            .with("host", "node-1")
            .with("env", "prod")
            .with("region", "us-east-1")
            .build();
        assert_eq!(set.len(), 3);
        assert_eq!(set.get("host").and_then(|v| v.as_str()), Some("node-1"));
    }

    #[test]
    fn attribute_set_sorted_keys() {
        let set = AttributeSetBuilder::new()
            .with("zzz", "last")
            .with("aaa", "first")
            .with("mmm", "middle")
            .build();
        let keys: Vec<&str> = set.keys().collect();
        assert_eq!(keys, vec!["aaa", "mmm", "zzz"]);
    }

    #[test]
    fn attribute_set_deduplication_first_wins() {
        let set = AttributeSetBuilder::new()
            .with("key", "first")
            .with("key", "second")
            .with("key", "third")
            .build();
        assert_eq!(set.len(), 1);
        assert_eq!(set.get("key").and_then(|v| v.as_str()), Some("first"));
    }

    #[test]
    fn attribute_set_clone_is_cheap() {
        let set = AttributeSetBuilder::new().with("host", "node-1").with("env", "prod").build();
        let set2 = set.clone();
        assert_eq!(set, set2);
        assert!(Arc::ptr_eq(&set.inner, &set2.inner));
    }

    #[test]
    fn attribute_set_hash_consistent() {
        let s1 = AttributeSetBuilder::new().with("host", "n1").with("env", "p").build();
        let s2 = AttributeSetBuilder::new().with("host", "n1").with("env", "p").build();
        assert_eq!(s1.hash(), s2.hash());
        assert_eq!(s1, s2);
    }

    #[test]
    fn attribute_set_typed_values() {
        let set = AttributeSetBuilder::new()
            .with("status", 200i64)
            .with("error", false)
            .with("p99", 0.95)
            .build();
        assert_eq!(set.get("status").and_then(|v| v.as_int()), Some(200));
        assert_eq!(set.get("error").and_then(|v| v.as_bool()), Some(false));
        assert!((set.get("p99").and_then(|v| v.as_float()).unwrap() - 0.95).abs() < 0.001);
    }

    #[test]
    fn attribute_set_contains_key() {
        let set = AttributeSetBuilder::new().with("host", "n1").build();
        assert!(set.contains_key("host"));
        assert!(!set.contains_key("nonexistent"));
    }

    #[test]
    fn attribute_set_from_attrset() {
        let mut attrs = AttrSet::new();
        attrs.push((
            Cow::Borrowed("host"),
            AttrValue::Str(Cow::Borrowed("node-1")),
        ));
        let set = AttributeSet::from(&attrs);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn attribute_set_to_attrset_roundtrip() {
        let set = AttributeSetBuilder::new().with("host", "n1").build();
        let attrs = set.to_attrset();
        let set2 = AttributeSet::from(&attrs);
        assert_eq!(set, set2);
    }

    #[test]
    fn attribute_set_extend_from_attrset() {
        let mut base = AttrSet::new();
        base.push((
            Cow::Borrowed("host"),
            AttrValue::Str(Cow::Borrowed("node-1")),
        ));
        let set = AttributeSetBuilder::new()
            .extend_from_attrset(&base)
            .with("env", "prod")
            .build();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn attribute_set_extend_from_set() {
        let base = AttributeSetBuilder::new().with("host", "n1").build();
        let set = AttributeSetBuilder::new().extend_from_set(&base).with("env", "prod").build();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn attribute_set_default_is_empty() {
        assert!(AttributeSet::default().is_empty());
    }

    #[test]
    fn attribute_set_from_static_slice() {
        let set: AttributeSet = [
            ("host", AttrValue::from("node-1")),
            ("env", AttrValue::from("prod")),
        ]
        .as_ref()
        .into();
        assert_eq!(set.len(), 2);
        assert_eq!(set.get("host").and_then(|v| v.as_str()), Some("node-1"));
    }

    #[test]
    fn attribute_set_from_vec() {
        let set: AttributeSet = vec![
            ("host".to_string(), AttrValue::from("node-1")),
            ("env".to_string(), AttrValue::from("prod")),
        ]
        .into();
        assert_eq!(set.len(), 2);
    }

    // ── merge ────────────────────────────────────────────────────

    #[test]
    fn attribute_set_merge_disjoint() {
        let a = AttributeSetBuilder::new().with("host", "n1").build();
        let b = AttributeSetBuilder::new().with("env", "p").build();
        let m = a.merge(&b);
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("host").and_then(|v| v.as_str()), Some("n1"));
        assert_eq!(m.get("env").and_then(|v| v.as_str()), Some("p"));
    }

    #[test]
    fn attribute_set_merge_override() {
        let a = AttributeSetBuilder::new().with("env", "staging").build();
        let b = AttributeSetBuilder::new().with("env", "prod").build();
        assert_eq!(a.merge(&b).len(), 1);
    }

    // ── contains_all ─────────────────────────────────────────────

    #[test]
    fn attribute_set_contains_all_subset() {
        let big = AttributeSetBuilder::new()
            .with("host", "n1")
            .with("env", "p")
            .with("region", "e")
            .build();
        let small = AttributeSetBuilder::new().with("host", "n1").with("env", "p").build();
        assert!(big.contains_all(&small));
    }

    #[test]
    fn attribute_set_contains_all_different_value() {
        let big = AttributeSetBuilder::new().with("env", "prod").build();
        let small = AttributeSetBuilder::new().with("env", "staging").build();
        assert!(!big.contains_all(&small));
    }

    #[test]
    fn attribute_set_contains_all_larger_subset() {
        let big = AttributeSetBuilder::new().with("host", "n1").build();
        let small = AttributeSetBuilder::new().with("host", "n1").with("env", "p").build();
        assert!(!big.contains_all(&small));
    }

    // ── PartialEq fast-path ──────────────────────────────────────

    #[test]
    fn attribute_set_eq_uses_hash_fastpath() {
        let s1 = AttributeSetBuilder::new().with("host", "n1").with("env", "p").build();
        let s2 = AttributeSetBuilder::new().with("host", "n1").with("env", "p").build();
        assert_eq!(s1, s2);
        assert!(!Arc::ptr_eq(&s1.inner, &s2.inner));
    }

    #[test]
    fn attribute_set_eq_same_arc_is_identity() {
        let s = AttributeSetBuilder::new().with("host", "n1").build();
        let s2 = s.clone();
        assert!(Arc::ptr_eq(&s.inner, &s2.inner));
        assert_eq!(s, s2);
    }

    #[test]
    fn fnv_hash_different_discriminants_produce_different_hashes() {
        let p1 = vec![(Cow::Borrowed("x"), AttrValue::Int(1))];
        let p2 = vec![(Cow::Borrowed("x"), AttrValue::Float(1.0))];
        assert_ne!(
            AttributeSetBuilder::fnv_hash(&p1),
            AttributeSetBuilder::fnv_hash(&p2)
        );
    }

    // ── BatchMetadata shared attrs ───────────────────────────────

    #[test]
    fn batch_metadata_with_shared_attrs() {
        let shared = AttributeSetBuilder::new()
            .with("collector.name", "ebpf")
            .with("collector.version", "1.0")
            .build();
        let meta = BatchMetadata::new("c1").with_shared_attrs(shared.clone());
        assert!(meta.shared_attributes.is_some());
        let sa = meta.shared_attributes.as_ref().unwrap();
        assert_eq!(
            sa.get("collector.name").and_then(|v| v.as_str()),
            Some("ebpf")
        );
        let meta2 = meta.clone();
        let sa2 = meta2.shared_attributes.as_ref().unwrap();
        assert!(Arc::ptr_eq(&sa.inner, &sa2.inner));
    }

    // ── Resource attributes_set ───────────────────────────────────

    #[test]
    fn resource_to_attribute_set() {
        let r = Resource::with_service("zerotrace")
            .with("host.name", "node-1")
            .with("deployment.environment", "production");
        let set = r.attributes_set();
        assert_eq!(set.len(), 2);
        assert_eq!(
            set.get("host.name").and_then(|v| v.as_str()),
            Some("node-1")
        );
    }
}
