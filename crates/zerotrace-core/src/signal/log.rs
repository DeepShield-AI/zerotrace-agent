// ── LogRecord — logging with trace correlation (OTEL §LogRecord) ──────

use super::{
    SignalType,
    attributes::{AttrSet, AttrValue},
    kind::SignalKind,
};
use std::{borrow::Cow, fmt};

// ═══════════════════════════════════════════════════════════════════════
// Severity
// ═══════════════════════════════════════════════════════════════════════

/// Severity of a log record.  Numeric values align with OTEL §SeverityNumber (range 1–24).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Trace = 1,
    Debug = 5,
    Info = 9,
    Warn = 13,
    Error = 17,
    Fatal = 21,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// LogRecord
// ═══════════════════════════════════════════════════════════════════════

/// A log record with trace/span correlation fields.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRecord {
    pub severity: Severity,
    pub severity_text: Cow<'static, str>,
    pub body: Cow<'static, str>,
    pub timestamp_ns: i64,
    pub observed_timestamp_ns: Option<i64>,
    pub trace_id: Option<[u8; 16]>,
    pub span_id: Option<[u8; 8]>,
    pub attributes: AttrSet,
}

impl LogRecord {
    pub fn new(severity: Severity, body: impl Into<Cow<'static, str>>, ts_ns: i64) -> Self {
        Self {
            severity,
            severity_text: Cow::Borrowed(severity.as_str()),
            body: body.into(),
            timestamp_ns: ts_ns,
            observed_timestamp_ns: None,
            trace_id: None,
            span_id: None,
            attributes: AttrSet::new(),
        }
    }

    pub fn with_trace(mut self, trace_id: [u8; 16], span_id: [u8; 8]) -> Self {
        self.trace_id = Some(trace_id);
        self.span_id = Some(span_id);
        self
    }
    pub fn with_observed_ts(mut self, ts_ns: i64) -> Self {
        self.observed_timestamp_ns = Some(ts_ns);
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
}

impl SignalType for LogRecord {
    fn signal_kind() -> SignalKind {
        SignalKind::LOG
    }
    fn estimated_heap_bytes(&self) -> usize {
        self.attributes.capacity() * std::mem::size_of::<(Cow<'static, str>, AttrValue)>()
    }
}
