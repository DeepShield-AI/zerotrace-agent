// ── Attribute system: AttrValue, AttrSet, AttributeSet ───────────────
//
// Layered design:
//   AttrValue  — typed value (Str|Int|Float|Bool), 24 bytes
//   AttrSet    — SmallVec<[(Cow, AttrValue); 4]>, mutable, inline up to 4 pairs
//   AttributeSet — Arc<sorted, deduped, pre-hashed pairs>, immutable, shared
//
// See module-level docs for design rationale and OTEL alignment.

use smallvec::SmallVec;
use std::{borrow::Cow, fmt, sync::Arc};

// ═══════════════════════════════════════════════════════════════════════
// AttrValue
// ═══════════════════════════════════════════════════════════════════════

/// A typed attribute value.
///
/// OTEL attributes are not always strings.  A metric label like
/// `http.status_code=200` is semantically an integer; `is_error=true`
/// is a boolean.  Storing everything as `String` loses type information.
///
/// # Float equality
///
/// `f64::NAN == f64::NAN` is `false` in Rust, which would break
/// `AttributeSet` equality (two logically identical sets would not
/// compare equal).  The manual [`PartialEq`] implementation treats
/// `NaN` as equal to `NaN` for attribute-comparison purposes.
#[derive(Debug, Clone)]
pub enum AttrValue {
    Str(Cow<'static, str>),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl PartialEq for AttrValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) =>
                if a.is_nan() && b.is_nan() {
                    true
                } else {
                    a == b
                },
            (Self::Bool(a), Self::Bool(b)) => a == b,
            _ => false,
        }
    }
}

impl AttrValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl fmt::Display for AttrValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Str(s) => write!(f, "{}", s),
            Self::Int(i) => write!(f, "{}", i),
            Self::Float(v) => write!(f, "{}", v),
            Self::Bool(b) => write!(f, "{}", b),
        }
    }
}

impl From<&'static str> for AttrValue {
    fn from(s: &'static str) -> Self {
        Self::Str(Cow::Borrowed(s))
    }
}
impl From<String> for AttrValue {
    fn from(s: String) -> Self {
        Self::Str(Cow::Owned(s))
    }
}
impl From<i64> for AttrValue {
    fn from(i: i64) -> Self {
        Self::Int(i)
    }
}
impl From<f64> for AttrValue {
    fn from(f: f64) -> Self {
        Self::Float(f)
    }
}
impl From<bool> for AttrValue {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// AttrSet — mutable inline storage
// ═══════════════════════════════════════════════════════════════════════

/// Mutable inline attribute storage.  4 pairs on the stack; spills to heap beyond.
///
/// For stable attribute sets shared across many signals, use [`AttributeSet`]
/// instead — it wraps the data in an `Arc`, eliminating per-signal cloning.
pub type AttrSet = SmallVec<[(Cow<'static, str>, AttrValue); 4]>;

// ═══════════════════════════════════════════════════════════════════════
// AttributeSet — Arc-backed immutable shared set
// ═══════════════════════════════════════════════════════════════════════

/// Internal sorted, deduplicated storage.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AttributeSetInner {
    pairs: Box<[(Cow<'static, str>, AttrValue)]>,
    hash: u64,
}

/// An immutable, Arc-backed set of attributes — the primary mechanism for
/// sharing stable attribute sets across many signals.  Create once via
/// [`AttributeSetBuilder`], clone cheaply (one ref-count bump).
///
/// | Approach | 1M signals, 4 attrs each | Allocations |
/// |---|---|---|
/// | `AttrSet` (SmallVec clone) | **192 MB** heap | 1,000,000 |
/// | `AttributeSet` (Arc share) | **0.2 KB** heap | 1 |
#[derive(Debug, Clone)]
pub struct AttributeSet {
    pub(crate) inner: Arc<AttributeSetInner>,
}

impl AttributeSet {
    pub fn len(&self) -> usize {
        self.inner.pairs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.inner.pairs.is_empty()
    }
    pub fn hash(&self) -> u64 {
        self.inner.hash
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Cow<'static, str>, AttrValue)> {
        self.inner.pairs.iter()
    }

    pub fn get(&self, key: &str) -> Option<&AttrValue> {
        self.inner
            .pairs
            .binary_search_by(|(k, _)| k.as_ref().cmp(key))
            .ok()
            .map(|i| &self.inner.pairs[i].1)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn contains_all(&self, other: &AttributeSet) -> bool {
        if other.len() > self.len() {
            return false;
        }
        other.iter().all(|(k, v)| self.get(k) == Some(v))
    }

    /// Merge two attribute sets.  When both sets contain the same key,
    /// **`self`'s value wins** (the "first" set's value is kept after
    /// stable-sort + dedup).  If you need the other set to override,
    /// reverse the call: `other.merge(self)`.
    pub fn merge(&self, other: &AttributeSet) -> AttributeSet {
        let mut builder = AttributeSetBuilder::with_capacity(self.len() + other.len());
        builder.pairs.extend(self.inner.pairs.iter().cloned());
        builder.pairs.extend(other.inner.pairs.iter().cloned());
        builder.build()
    }

    pub fn to_attrset(&self) -> AttrSet {
        self.inner.pairs.iter().cloned().collect()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.inner.pairs.iter().map(|(k, _)| k.as_ref())
    }

    pub fn has_value(&self, key: &str, needle: &str) -> bool {
        matches!(self.get(key), Some(AttrValue::Str(s)) if s == needle)
    }
}

impl PartialEq for AttributeSet {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.inner, &other.inner) {
            return true;
        }
        if self.inner.hash != other.inner.hash {
            return false;
        }
        self.inner.pairs == other.inner.pairs
    }
}
impl Eq for AttributeSet {}

impl std::hash::Hash for AttributeSet {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.inner.hash);
    }
}

impl Default for AttributeSet {
    fn default() -> Self {
        AttributeSetBuilder::new().build()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// AttributeSetBuilder
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct AttributeSetBuilder {
    pub(crate) pairs: Vec<(Cow<'static, str>, AttrValue)>,
}

impl AttributeSetBuilder {
    pub fn new() -> Self {
        Self { pairs: Vec::new() }
    }
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            pairs: Vec::with_capacity(cap),
        }
    }

    pub fn with(mut self, key: impl Into<Cow<'static, str>>, val: impl Into<AttrValue>) -> Self {
        self.pairs.push((key.into(), val.into()));
        self
    }

    pub fn extend_from_attrset(mut self, other: &AttrSet) -> Self {
        self.pairs.extend(other.iter().cloned());
        self
    }

    pub fn extend_from_set(mut self, other: &AttributeSet) -> Self {
        self.pairs.extend(other.iter().cloned());
        self
    }

    pub fn build(mut self) -> AttributeSet {
        self.pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
        self.pairs.dedup_by(|(a, _), (b, _)| a == b);
        let hash = Self::fnv_hash(&self.pairs);
        AttributeSet {
            inner: Arc::new(AttributeSetInner {
                pairs: self.pairs.into_boxed_slice(),
                hash,
            }),
        }
    }

    /// FNV-1a 64-bit hash over sorted pairs.  Hashes discriminant + raw bytes directly.
    pub(crate) fn fnv_hash(pairs: &[(Cow<'static, str>, AttrValue)]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for (k, v) in pairs {
            for b in k.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            h ^= 0xff;
            match v {
                AttrValue::Str(s) => {
                    h ^= 0x01;
                    h = h.wrapping_mul(0x100000001b3);
                    for b in s.as_bytes() {
                        h ^= *b as u64;
                        h = h.wrapping_mul(0x100000001b3);
                    }
                },
                AttrValue::Int(i) => {
                    h ^= 0x02;
                    h = h.wrapping_mul(0x100000001b3);
                    for b in i.to_le_bytes() {
                        h ^= b as u64;
                        h = h.wrapping_mul(0x100000001b3);
                    }
                },
                AttrValue::Float(f) => {
                    h ^= 0x03;
                    h = h.wrapping_mul(0x100000001b3);
                    for b in f.to_le_bytes() {
                        h ^= b as u64;
                        h = h.wrapping_mul(0x100000001b3);
                    }
                },
                AttrValue::Bool(b) => {
                    h ^= 0x04;
                    h = h.wrapping_mul(0x100000001b3);
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                },
            }
        }
        h
    }
}

impl Default for AttributeSetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone + Into<Cow<'static, str>>, V: Clone + Into<AttrValue>> From<&[(K, V)]>
    for AttributeSet
{
    fn from(slice: &[(K, V)]) -> Self {
        let mut builder = AttributeSetBuilder::with_capacity(slice.len());
        for (k, v) in slice {
            builder.pairs.push((k.clone().into(), v.clone().into()));
        }
        builder.build()
    }
}

impl<K: Into<Cow<'static, str>>, V: Into<AttrValue>> From<Vec<(K, V)>> for AttributeSet {
    fn from(vec: Vec<(K, V)>) -> Self {
        let mut builder = AttributeSetBuilder::with_capacity(vec.len());
        for (k, v) in vec {
            builder.pairs.push((k.into(), v.into()));
        }
        builder.build()
    }
}

impl From<AttrSet> for AttributeSet {
    fn from(attrs: AttrSet) -> Self {
        let mut builder = AttributeSetBuilder::with_capacity(attrs.len());
        builder.pairs = attrs.into_vec();
        builder.build()
    }
}

impl From<&AttrSet> for AttributeSet {
    fn from(attrs: &AttrSet) -> Self {
        let mut builder = AttributeSetBuilder::with_capacity(attrs.len());
        builder.pairs.extend(attrs.iter().cloned());
        builder.build()
    }
}
