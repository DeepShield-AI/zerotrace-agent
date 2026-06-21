// ── Resource — shared identity context (OTEL §Resource) ─────────────

use super::attributes::{AttrSet, AttrValue, AttributeSet};
use std::borrow::Cow;

/// Identifies the entity producing telemetry.
///
/// A `Resource` carries attributes like `service.name`, `host.name`, and
/// `deployment.environment`.  Created once per process and attached to
/// every batch via an [`Arc`].
///
/// Per the OTEL spec, a Resource MUST have at least `service.name`.
/// `Resource::with_service()` enforces this at the type level.
#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    pub service_name: Cow<'static, str>,
    pub attributes: AttrSet,
}

impl Resource {
    pub fn with_service(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            service_name: name.into(),
            attributes: AttrSet::new(),
        }
    }

    pub fn with(mut self, key: impl Into<Cow<'static, str>>, val: impl Into<AttrValue>) -> Self {
        self.attributes.push((key.into(), val.into()));
        self
    }

    pub fn attributes_set(&self) -> AttributeSet {
        AttributeSet::from(&self.attributes)
    }

    pub fn estimated_heap_bytes(&self) -> usize {
        self.attributes.capacity() * std::mem::size_of::<(Cow<'static, str>, AttrValue)>()
    }
}
