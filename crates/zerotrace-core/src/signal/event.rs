// ── SystemEvent — structured events (OTEL §Event) ────────────────────

use super::{
    SignalType,
    attributes::{AttrSet, AttrValue},
    kind::SignalKind,
};
use std::borrow::Cow;

/// A structured event record.
/// Semantically equivalent to LogRecords with `event.domain` and `event.name`
/// attributes (per OTEL semantic conventions).
#[derive(Debug, Clone, PartialEq)]
pub struct SystemEvent {
    pub domain: Cow<'static, str>,
    pub name: Cow<'static, str>,
    pub payload: Cow<'static, str>,
    pub timestamp_ns: i64,
    pub attributes: AttrSet,
}

impl SystemEvent {
    pub fn new(
        domain: impl Into<Cow<'static, str>>,
        name: impl Into<Cow<'static, str>>,
        payload: impl Into<Cow<'static, str>>,
        ts_ns: i64,
    ) -> Self {
        Self {
            domain: domain.into(),
            name: name.into(),
            payload: payload.into(),
            timestamp_ns: ts_ns,
            attributes: AttrSet::new(),
        }
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

impl SignalType for SystemEvent {
    fn signal_kind() -> SignalKind {
        SignalKind::EVENT
    }
}
