// ── Batch types — transport containers for signals ────────────────────

use super::{
    AnySignal, SignalType, attributes::AttributeSet, kind::SignalKind, resource::Resource,
};
use std::{borrow::Cow, sync::Arc};

// ═══════════════════════════════════════════════════════════════════════
// BatchMetadata
// ═══════════════════════════════════════════════════════════════════════

/// Batch metadata — allocated once, shared via `Arc`.
///
/// Carries both [`Resource`] and shared [`AttributeSet`].  Every signal
/// in the batch logically inherits these attributes.
#[derive(Debug, Clone)]
pub struct BatchMetadata {
    pub resource: Option<Arc<Resource>>,
    pub shared_attributes: Option<AttributeSet>,
    pub source_id: Cow<'static, str>,
    pub host: Cow<'static, str>,
    pub created_ns: i64,
    pub deadline_ns: Option<i64>,
}

impl BatchMetadata {
    pub fn new(source_id: impl Into<Cow<'static, str>>) -> Self {
        Self {
            resource: None,
            shared_attributes: None,
            source_id: source_id.into(),
            host: Cow::Borrowed(""),
            created_ns: 0,
            deadline_ns: None,
        }
    }
    pub fn with_resource(mut self, r: Arc<Resource>) -> Self {
        self.resource = Some(r);
        self
    }
    pub fn with_shared_attrs(mut self, a: AttributeSet) -> Self {
        self.shared_attributes = Some(a);
        self
    }
}

impl std::hash::Hash for BatchMetadata {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.source_id.hash(state);
        self.host.hash(state);
        self.created_ns.hash(state);
        self.deadline_ns.hash(state);
        self.resource.as_ref().map(|r| &r.service_name).hash(state);
        self.shared_attributes.hash(state); // delegates to AttributeSet::hash
    }
}

impl PartialEq for BatchMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.source_id == other.source_id &&
            self.host == other.host &&
            self.created_ns == other.created_ns &&
            self.deadline_ns == other.deadline_ns &&
            self.resource.as_ref().map(|r| &r.service_name) ==
                other.resource.as_ref().map(|r| &r.service_name) &&
            self.shared_attributes == other.shared_attributes // delegates to AttributeSet::eq (hash fast-path + full compare)
    }
}
impl Eq for BatchMetadata {}

// ═══════════════════════════════════════════════════════════════════════
// TypedBatch<T>
// ═══════════════════════════════════════════════════════════════════════

/// A type-safe, homogeneous batch of signals.
/// `TypedBatch<MetricPoint>` can only contain `MetricPoint` values.
#[derive(Debug, Clone)]
pub struct TypedBatch<T: SignalType> {
    pub items: Vec<T>,
    pub metadata: Arc<BatchMetadata>,
}

impl<T: SignalType> TypedBatch<T> {
    pub fn new(metadata: Arc<BatchMetadata>) -> Self {
        Self {
            items: Vec::new(),
            metadata,
        }
    }
    pub fn with_capacity(metadata: Arc<BatchMetadata>, cap: usize) -> Self {
        Self {
            items: Vec::with_capacity(cap),
            metadata,
        }
    }
    pub fn from_vec(items: Vec<T>, metadata: Arc<BatchMetadata>) -> Self {
        Self { items, metadata }
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn push(&mut self, item: T) {
        self.items.push(item);
    }
    pub fn drain(&mut self) -> Vec<T> {
        std::mem::take(&mut self.items)
    }
    pub fn kind() -> SignalKind {
        T::signal_kind()
    }
    pub fn estimated_heap_bytes(&self) -> usize {
        std::mem::size_of::<Self>() +
            self.items.capacity() * std::mem::size_of::<T>() +
            self.items.iter().map(|i| i.estimated_heap_bytes()).sum::<usize>()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Batch — heterogeneous batch
// ═══════════════════════════════════════════════════════════════════════

/// A heterogeneous batch of type-erased signals — the primary transport
/// type for pipeline channels.  Each item carries its own kind.
#[derive(Debug, Clone)]
pub struct Batch {
    pub items: Vec<AnySignal>,
    pub metadata: Arc<BatchMetadata>,
}

impl Batch {
    pub fn new(metadata: Arc<BatchMetadata>) -> Self {
        Self {
            items: Vec::new(),
            metadata,
        }
    }
    pub fn with_capacity(metadata: Arc<BatchMetadata>, cap: usize) -> Self {
        Self {
            items: Vec::with_capacity(cap),
            metadata,
        }
    }
    pub fn from_items(items: Vec<AnySignal>) -> Self {
        Self {
            items,
            metadata: Arc::new(BatchMetadata::new("unnamed")),
        }
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn push<T: SignalType>(&mut self, item: T) {
        self.items.push(AnySignal::new(item));
    }
    pub fn push_any(&mut self, item: AnySignal) {
        self.items.push(item);
    }
    pub fn drain(&mut self) -> Vec<AnySignal> {
        std::mem::take(&mut self.items)
    }
    pub fn filter<T: SignalType>(&self) -> Vec<&T> {
        self.items.iter().filter_map(|a| a.downcast::<T>()).collect()
    }
    pub fn all<T: SignalType>(&self) -> bool {
        self.items.iter().all(|a| a.is::<T>())
    }
    pub fn into_typed<T: SignalType>(self) -> Option<TypedBatch<T>> {
        let mut items = Vec::with_capacity(self.items.len());
        for any in self.items {
            match any.downcast::<T>() {
                Some(item) => items.push(item.clone()),
                None => return None,
            }
        }
        Some(TypedBatch {
            items,
            metadata: self.metadata,
        })
    }
    pub fn into_typed_lossy<T: SignalType>(self) -> TypedBatch<T> {
        let items: Vec<T> = self.items.iter().filter_map(|a| a.downcast::<T>().cloned()).collect();
        TypedBatch {
            items,
            metadata: self.metadata,
        }
    }
    pub fn estimated_heap_bytes(&self) -> usize {
        self.items.iter().map(|a| a.estimated_heap_bytes()).sum()
    }
}
