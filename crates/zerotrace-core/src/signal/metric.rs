// ── MetricValue, MetricPoint — metrics data model (OTEL §Metric) ─────

use super::{
    SignalType,
    attributes::{AttrSet, AttrValue},
    kind::SignalKind,
};
use smallvec::SmallVec;
use std::borrow::Cow;

// ═══════════════════════════════════════════════════════════════════════
// MetricValue
// ═══════════════════════════════════════════════════════════════════════

/// The mathematical behavior of a metric.
///
/// | Variant | OTEL Instrument | Behavior |
/// |---|---|---|
/// | `Gauge(f64)` | Gauge | Point-in-time snapshot |
/// | `Counter(f64)` | Sum (monotonic) | Cumulative, monotonically increasing |
/// | `Histogram { ... }` | Histogram | Statistical distribution with buckets |
#[derive(Debug, Clone, PartialEq)]
pub enum MetricValue {
    Gauge(f64),
    Counter(f64),
    Histogram {
        bucket_counts: SmallVec<[u64; 8]>,
        bucket_bounds: SmallVec<[f64; 8]>,
        sum: f64,
        count: u64,
    },
}

impl MetricValue {
    pub fn is_monotonic(&self) -> bool {
        matches!(self, Self::Counter(_))
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Gauge(v) | Self::Counter(v) => Some(*v),
            Self::Histogram { .. } => None,
        }
    }

    /// Construct a Histogram from parallel arrays of bucket bounds and counts.
    pub fn histogram(bounds: &[f64], counts: &[u64], sum: f64, total_count: u64) -> Self {
        Self::Histogram {
            bucket_counts: counts.iter().copied().collect(),
            bucket_bounds: bounds.iter().copied().collect(),
            sum,
            count: total_count,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// MetricPoint
// ═══════════════════════════════════════════════════════════════════════

/// A single metric data point.
///
/// Every `MetricPoint` has:
///   - `name` — the metric name (e.g. `"http.server.request.duration"`)
///   - `value` — the [`MetricValue`] (Gauge, Counter, or Histogram)
///   - `timestamp_ns` — Unix epoch nanoseconds
///
/// Optional: description, unit, attributes, start_time.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricPoint {
    pub name: Cow<'static, str>,
    pub description: Option<Cow<'static, str>>,
    pub unit: Cow<'static, str>,
    pub value: MetricValue,
    pub attributes: AttrSet,
    pub timestamp_ns: i64,
    pub start_time_ns: Option<i64>,
}

impl MetricPoint {
    pub fn gauge(name: impl Into<Cow<'static, str>>, value: f64, ts_ns: i64) -> Self {
        Self {
            name: name.into(),
            description: None,
            unit: Cow::Borrowed(""),
            value: MetricValue::Gauge(value),
            attributes: AttrSet::new(),
            timestamp_ns: ts_ns,
            start_time_ns: None,
        }
    }

    pub fn counter(name: impl Into<Cow<'static, str>>, value: f64, ts_ns: i64) -> Self {
        Self {
            name: name.into(),
            description: None,
            unit: Cow::Borrowed(""),
            value: MetricValue::Counter(value),
            attributes: AttrSet::new(),
            timestamp_ns: ts_ns,
            start_time_ns: None,
        }
    }

    pub fn histogram(
        name: impl Into<Cow<'static, str>>,
        bounds: &[f64],
        counts: &[u64],
        sum: f64,
        total: u64,
        ts_ns: i64,
    ) -> Self {
        Self {
            name: name.into(),
            description: None,
            unit: Cow::Borrowed(""),
            value: MetricValue::histogram(bounds, counts, sum, total),
            attributes: AttrSet::new(),
            timestamp_ns: ts_ns,
            start_time_ns: None,
        }
    }

    pub fn with_description(mut self, d: impl Into<Cow<'static, str>>) -> Self {
        self.description = Some(d.into());
        self
    }
    pub fn with_unit(mut self, u: impl Into<Cow<'static, str>>) -> Self {
        self.unit = u.into();
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
    pub fn with_start_time(mut self, ts_ns: i64) -> Self {
        self.start_time_ns = Some(ts_ns);
        self
    }
}

impl SignalType for MetricPoint {
    fn signal_kind() -> SignalKind {
        SignalKind::METRIC
    }
    fn estimated_heap_bytes(&self) -> usize {
        self.attributes.capacity() * std::mem::size_of::<(Cow<'static, str>, AttrValue)>()
    }
}
