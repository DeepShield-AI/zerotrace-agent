// ── Span — distributed tracing (OTEL §Span) ──────────────────────────

use super::{
    SignalType,
    attributes::{AttrSet, AttrValue},
    kind::SignalKind,
};
use std::borrow::Cow;

// ═══════════════════════════════════════════════════════════════════════
// SpanKind
// ═══════════════════════════════════════════════════════════════════════

/// The role of a span in a distributed trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpanKind {
    Internal,
    Server,
    Client,
    Producer,
    Consumer,
}

// ═══════════════════════════════════════════════════════════════════════
// SpanStatus
// ═══════════════════════════════════════════════════════════════════════

/// The outcome of a span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanStatus {
    Ok,
    Error { message: Cow<'static, str> },
}

// ═══════════════════════════════════════════════════════════════════════
// Span
// ═══════════════════════════════════════════════════════════════════════

/// A span representing a single operation within a distributed trace.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub parent_span_id: Option<[u8; 8]>,
    pub trace_flags: u8,
    pub kind: SpanKind,
    pub service_name: Cow<'static, str>,
    pub name: Cow<'static, str>,
    pub status: Option<SpanStatus>,
    pub start_ns: i64,
    pub duration_ns: i64,
    pub attributes: AttrSet,
}

impl Span {
    pub fn new(
        trace_id: [u8; 16],
        span_id: [u8; 8],
        service_name: impl Into<Cow<'static, str>>,
        name: impl Into<Cow<'static, str>>,
        start_ns: i64,
    ) -> Self {
        Self {
            trace_id,
            span_id,
            parent_span_id: None,
            trace_flags: 1,
            kind: SpanKind::Internal,
            service_name: service_name.into(),
            name: name.into(),
            status: None,
            start_ns,
            duration_ns: 0,
            attributes: AttrSet::new(),
        }
    }

    pub fn with_parent(mut self, pid: [u8; 8]) -> Self {
        self.parent_span_id = Some(pid);
        self
    }
    pub fn with_kind(mut self, k: SpanKind) -> Self {
        self.kind = k;
        self
    }
    pub fn with_status(mut self, s: SpanStatus) -> Self {
        self.status = Some(s);
        self
    }
    pub fn with_attr(
        mut self,
        key: impl Into<Cow<'static, str>>,
        val: impl Into<AttrValue>,
    ) -> Self {
        self.attributes.push((key.into(), val.into()));
        self
    }
    pub fn duration_secs(&self) -> f64 {
        self.duration_ns as f64 / 1e9
    }
}

impl SignalType for Span {
    fn signal_kind() -> SignalKind {
        SignalKind::TRACE
    }
    fn estimated_heap_bytes(&self) -> usize {
        self.attributes.capacity() * std::mem::size_of::<(Cow<'static, str>, AttrValue)>()
    }
}
