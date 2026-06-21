// ── SignalKind — open identifier for signal routing ──────────────────
//
// Replaces the old closed enum.  Any crate can define `SignalKind("ai.anomaly")`
// without modifying core.  `TypeId`-based routing in `AnySignal` provides the
// fast path; the kind name is the human-readable fallback.

use std::fmt;

/// Identifies the category of a signal for routing and filtering.
///
/// # Why not an enum?
///
/// An enum is closed.  Every new signal type requires a PR to `zerotrace-core`.
/// With a struct + constants, plugins define their own kinds without modifying
/// core.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SignalKind(pub &'static str);

impl SignalKind {
    /// Built-in kinds aligned with OpenTelemetry.
    pub const METRIC: Self = Self("metric");
    pub const TRACE: Self = Self("trace");
    pub const LOG: Self = Self("log");
    pub const PROFILE: Self = Self("profile");
    pub const EVENT: Self = Self("event");

    pub fn as_str(&self) -> &str {
        self.0
    }
}

impl fmt::Display for SignalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl From<&'static str> for SignalKind {
    fn from(s: &'static str) -> Self {
        Self(s)
    }
}
