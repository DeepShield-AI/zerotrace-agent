use std::any::Any;
use std::sync::Arc;

pub trait ErasedSignal: Any + Send + Sync {
    fn kind_name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
}

#[derive(Debug, Clone)]
pub enum Signal {
    Metric(MetricPoint),
    Trace(Span),
    Log(LogRecord),
    Profile(StackSample),
    Event(SystemEvent),
    Custom(Arc<dyn ErasedSignal>),
}

impl Signal {
    pub fn kind(&self) -> SignalKind {
        match self {
            Signal::Metric(_) => SignalKind::Metric,
            Signal::Trace(_) => SignalKind::Trace,
            Signal::Log(_) => SignalKind::Log,
            Signal::Profile(_) => SignalKind::Profile,
            Signal::Event(_) => SignalKind::Event,
            Signal::Custom(c) => SignalKind::Custom(c.kind_name()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SignalKind {
    Metric, Trace, Log, Profile, Event,
    Custom(&'static str),
}

#[derive(Debug, Clone)]
pub struct MetricPoint {
    pub name: String,
    pub value: f64,
    pub tags: Vec<(String, String)>,
    pub timestamp_ns: i64,
}

#[derive(Debug, Clone)]
pub struct Span {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub parent_span_id: Option<[u8; 8]>,
    pub service_name: String,
    pub operation_name: String,
    pub start_ns: i64,
    pub duration_ns: i64,
    pub tags: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct LogRecord {
    pub level: LogLevel,
    pub message: String,
    pub timestamp_ns: i64,
    pub attributes: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel { Trace, Debug, Info, Warn, Error, Fatal }

#[derive(Debug, Clone)]
pub struct StackSample {
    pub process_name: String,
    pub pid: u32,
    pub stack_frames: Vec<u64>,
    pub count: u64,
}

#[derive(Debug, Clone)]
pub struct SystemEvent {
    pub event_type: String,
    pub payload: String,
    pub timestamp_ns: i64,
}

#[derive(Debug, Clone)]
pub struct SignalBatch {
    pub kind: SignalKind,
    pub items: Vec<Signal>,
    pub deadline_ns: Option<i64>,
}

impl SignalBatch {
    pub fn new(kind: SignalKind) -> Self {
        Self { kind, items: Vec::new(), deadline_ns: None }
    }
    pub fn len(&self) -> usize { self.items.len() }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug)] struct DummyAnomaly { pub score: f64 }
    impl ErasedSignal for DummyAnomaly {
        fn kind_name(&self) -> &'static str { "ai.anomaly" }
        fn as_any(&self) -> &dyn Any { self }
    }
    #[test] fn test_known_signal_kind() {
        let m = Signal::Metric(MetricPoint { name: "cpu".into(), value: 0.8, tags: vec![], timestamp_ns: 0 });
        assert_eq!(m.kind(), SignalKind::Metric);
    }
    #[test] fn test_custom_signal() {
        let s = Signal::Custom(Arc::new(DummyAnomaly { score: 0.95 }));
        assert_eq!(s.kind(), SignalKind::Custom("ai.anomaly"));
    }
}
