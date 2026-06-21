// World: Bevy-inspired resource container.
// Uses Arc<RwLock<T>> — safe, simple, and fast enough for agent orchestration.
// Bevy uses UnsafeCell for ECS-scale parallelism; our sequential scheduler
// doesn't need that complexity. See docs/bevy-comparison.md for details.
//
// # Keyed resources
//
// By default each type can only exist once in the World.  Use
// `insert_keyed::<K, T>(value)` to store multiple instances of the same
// value type `T` distinguished by a key type `K`.  Retrieve with
// `get_keyed::<K, T>()` and `ResKeyed<K, T>` SystemParam.
//
// ```rust
// use std::sync::Arc;
// use zerotrace_kernel::world::World;
//
// struct HttpClient(String);
// struct MetricsKey;  // marker type
// struct LogsKey;     // marker type
//
// let w = World::new();
// w.insert_keyed::<MetricsKey, HttpClient>(HttpClient("metrics.example.com".into()));
// w.insert_keyed::<LogsKey, HttpClient>(HttpClient("logs.example.com".into()));
// ```

use crate::{error::Result, metrics::KernelMetrics};
use parking_lot::{Mutex, RwLock};
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub type Tick = u64;

#[derive(Debug, Clone, Copy)]
pub struct SystemContext {
    pub this_run: Tick,
    pub last_run: Tick,
}
impl SystemContext {
    pub fn new(t: Tick, l: Tick) -> Self {
        Self {
            this_run: t,
            last_run: l,
        }
    }
}

pub struct ResourceMeta {
    pub added_tick: AtomicU64,
    pub changed_tick: AtomicU64,
}
impl ResourceMeta {
    pub fn new(tick: Tick) -> Self {
        Self {
            added_tick: AtomicU64::new(tick),
            changed_tick: AtomicU64::new(tick),
        }
    }
    pub fn snapshot(&self) -> ResourceMetaSnapshot {
        ResourceMetaSnapshot {
            added_tick: self.added_tick.load(Ordering::Acquire),
            changed_tick: self.changed_tick.load(Ordering::Acquire),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceMetaSnapshot {
    pub added_tick: Tick,
    pub changed_tick: Tick,
}

#[derive(Debug, Default)]
pub struct CommandQueue {
    pub(crate) buffer: Mutex<Vec<ErasedCommand>>,
}
impl CommandQueue {
    pub fn new() -> Self {
        Self {
            buffer: Mutex::new(Vec::new()),
        }
    }
}

#[derive(Debug)]
pub enum ErasedCommand {
    Insert(ResourceKey, Arc<dyn Any + Send + Sync>),
    Remove(ResourceKey),
}

/// Composite key for the resource map: `(type_id, optional_key)`.
///
/// - `key: None` — plain (un-keyed) resource.
/// - `key: Some(k)` — keyed resource, allowing multiple instances of the
///   same value type `T` distinguished by a key type `K`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceKey {
    pub type_id: TypeId,
    /// When `Some`, this is a keyed resource — the stored [`TypeId`] is
    /// `TypeId::of::<K>()` for the key type `K`, allowing multiple
    /// instances of the same value type `T` to coexist.
    pub key_type_id: Option<TypeId>,
}

impl ResourceKey {
    pub fn of<T: Any>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            key_type_id: None,
        }
    }
    pub fn keyed<K: Any, T: Any>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            key_type_id: Some(TypeId::of::<K>()),
        }
    }
}

type ResourceEntry = (Arc<dyn Any + Send + Sync>, Arc<ResourceMeta>);

pub struct World {
    resources: RwLock<HashMap<ResourceKey, ResourceEntry>>,
    change_tick: AtomicU64,
    /// Optional metrics sink.  When set, `get()` and `insert()` record
    /// latency and call counts.  Defaults to no-op.
    metrics: Arc<KernelMetrics>,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World").finish()
    }
}

impl World {
    pub fn new() -> Self {
        let w = Self {
            resources: RwLock::new(HashMap::new()),
            change_tick: AtomicU64::new(1),
            metrics: Arc::new(KernelMetrics::new()),
        };
        // CommandQueue and LifecycleRegistry use insert_raw (no outer
        // RwLock) because they have their own internal locking.
        let tick = w.next_tick();
        let m = Arc::new(ResourceMeta::new(tick));
        let cq: Arc<CommandQueue> = Arc::new(CommandQueue::new());
        w.resources.write().insert(ResourceKey::of::<CommandQueue>(), (cq, m));
        let tick = w.next_tick();
        let m = Arc::new(ResourceMeta::new(tick));
        let lr: Arc<crate::lifecycle::LifecycleRegistry> =
            Arc::new(crate::lifecycle::LifecycleRegistry::new());
        w.resources.write().insert(
            ResourceKey::of::<crate::lifecycle::LifecycleRegistry>(),
            (lr, m),
        );
        w
    }

    /// Replace the metrics sink.  Call this to enable framework
    /// self-observability (e.g. for debug socket or Prometheus export).
    pub fn set_metrics(&mut self, m: Arc<KernelMetrics>) {
        self.metrics = m;
    }

    /// Borrow the current metrics sink.
    pub fn metrics(&self) -> &Arc<KernelMetrics> {
        &self.metrics
    }

    pub fn current_tick(&self) -> Tick {
        self.change_tick.load(Ordering::Acquire)
    }
    pub(crate) fn next_tick(&self) -> Tick {
        self.change_tick.fetch_add(1, Ordering::AcqRel)
    }

    // ── Un-keyed insert / get (backward-compatible API) ────────────────

    /// Insert a resource wrapped in `Arc<RwLock<T>>` (the default).
    ///
    /// Returns `true` on first insert, `false` when a resource of the
    /// same type already existed (overwritten).  The overwrite is
    /// intentional — it supports hot-reload patterns — but callers can
    /// inspect the return value to detect accidental double-registration.
    ///
    /// On overwrite, the existing [`ResourceMeta`] is **reused** (its
    /// `changed_tick` is bumped) so that outstanding [`ResMut`] handles
    /// can detect the change via [`ResMut::is_changed`].
    pub fn insert<T: Any + Send + Sync>(&self, value: T) -> bool {
        let key = ResourceKey::of::<T>();
        let existed = {
            let map = self.resources.read();
            map.contains_key(&key)
        };
        if existed {
            tracing::warn!(
                "resource [{}] is being overwritten via World::insert()",
                std::any::type_name::<T>()
            );
            self.metrics
                .resource_overwrite_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Reuse the existing ResourceMeta so outstanding ResMut handles
            // can observe the change via is_changed().
            let mut map = self.resources.write();
            let meta = if let Some((_, existing_meta)) = map.get(&key) {
                let new_tick = self.next_tick();
                existing_meta.changed_tick.store(new_tick, Ordering::Release);
                existing_meta.added_tick.store(new_tick, Ordering::Release);
                existing_meta.clone()
            } else {
                // Race: resource was removed between check and write lock
                let tick = self.next_tick();
                Arc::new(ResourceMeta::new(tick))
            };
            map.insert(key, (Arc::new(RwLock::new(value)), meta));
            return false;
        }
        let tick = self.next_tick();
        let m = Arc::new(ResourceMeta::new(tick));
        self.resources.write().insert(key, (Arc::new(RwLock::new(value)), m));
        true
    }

    /// Insert a resource that already manages its own synchronisation.
    /// On overwrite, reuses the existing [`ResourceMeta`] so outstanding
    /// handles can detect the change.
    pub fn insert_raw<T: Any + Send + Sync>(&self, value: Arc<T>) {
        let key = ResourceKey::of::<T>();
        let mut map = self.resources.write();
        let meta = if let Some((_, existing_meta)) = map.get(&key) {
            let new_tick = self.next_tick();
            existing_meta.changed_tick.store(new_tick, Ordering::Release);
            existing_meta.added_tick.store(new_tick, Ordering::Release);
            existing_meta.clone()
        } else {
            let tick = self.next_tick();
            Arc::new(ResourceMeta::new(tick))
        };
        map.insert(key, (value, meta));
    }

    pub fn get<T: Any + Send + Sync>(&self) -> Result<(Arc<RwLock<T>>, ResourceMetaSnapshot)> {
        self._get(ResourceKey::of::<T>())
    }

    pub fn get_raw<T: Any + Send + Sync>(&self) -> Result<(Arc<T>, ResourceMetaSnapshot)> {
        self._get_raw(ResourceKey::of::<T>())
    }

    pub fn get_meta_arc<T: Any + Send + Sync>(
        &self,
    ) -> Result<(Arc<RwLock<T>>, Arc<ResourceMeta>)> {
        self._get_meta_arc(ResourceKey::of::<T>())
    }

    pub fn remove_resource<T: Any + Send + Sync>(&self) -> Option<Arc<RwLock<T>>> {
        self.next_tick();
        self.resources
            .write()
            .remove(&ResourceKey::of::<T>())
            .and_then(|(d, _)| d.downcast::<RwLock<T>>().ok())
    }

    pub fn contains<T: Any + Send + Sync>(&self) -> bool {
        self.resources.read().contains_key(&ResourceKey::of::<T>())
    }

    /// Check whether a resource with the given `TypeId` (un-keyed) exists.
    pub fn contains_tid(&self, tid: TypeId) -> bool {
        self.resources.read().contains_key(&ResourceKey {
            type_id: tid,
            key_type_id: None,
        })
    }

    // ── Keyed insert / get ─────────────────────────────────────────────

    /// Insert a keyed resource.  Multiple instances of the same value type
    /// `T` can coexist as long as they have different key types `K`.
    ///
    /// ```
    /// use zerotrace_kernel::world::World;
    ///
    /// struct HttpClient(String);
    /// struct MetricsKey;
    /// struct LogsKey;
    ///
    /// let world = World::new();
    /// world.insert_keyed::<MetricsKey, HttpClient>(HttpClient("metrics.example.com".into()));
    /// world.insert_keyed::<LogsKey, HttpClient>(HttpClient("logs.example.com".into()));
    /// ```
    pub fn insert_keyed<K: Any, T: Any + Send + Sync>(&self, value: T) {
        let tick = self.next_tick();
        let m = Arc::new(ResourceMeta::new(tick));
        self.resources.write().insert(
            ResourceKey::keyed::<K, T>(),
            (Arc::new(RwLock::new(value)), m),
        );
    }

    /// Insert a keyed raw resource (self-synchronising).
    pub fn insert_raw_keyed<K: Any, T: Any + Send + Sync>(&self, value: Arc<T>) {
        let tick = self.next_tick();
        let m = Arc::new(ResourceMeta::new(tick));
        self.resources.write().insert(ResourceKey::keyed::<K, T>(), (value, m));
    }

    /// Retrieve a keyed resource.
    pub fn get_keyed<K: Any, T: Any + Send + Sync>(
        &self,
    ) -> Result<(Arc<RwLock<T>>, ResourceMetaSnapshot)> {
        self._get(ResourceKey::keyed::<K, T>())
    }

    /// Retrieve a keyed raw resource.
    pub fn get_raw_keyed<K: Any, T: Any + Send + Sync>(
        &self,
    ) -> Result<(Arc<T>, ResourceMetaSnapshot)> {
        self._get_raw(ResourceKey::keyed::<K, T>())
    }

    /// Check whether a keyed resource exists.
    pub fn contains_keyed<K: Any, T: Any + Send + Sync>(&self) -> bool {
        self.resources.read().contains_key(&ResourceKey::keyed::<K, T>())
    }

    /// Remove a keyed resource.
    pub fn remove_keyed<K: Any, T: Any + Send + Sync>(&self) -> Option<Arc<RwLock<T>>> {
        self.next_tick();
        self.resources
            .write()
            .remove(&ResourceKey::keyed::<K, T>())
            .and_then(|(d, _)| d.downcast::<RwLock<T>>().ok())
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn _get<T: Any + Send + Sync>(
        &self,
        key: ResourceKey,
    ) -> Result<(Arc<RwLock<T>>, ResourceMetaSnapshot)> {
        let start = std::time::Instant::now();
        let map = self.resources.read();
        let (d, m) =
            map.get(&key).ok_or_else(|| zerotrace_core::error::Error::ResourceNotFound {
                type_name: std::any::type_name::<T>(),
            })?;
        let result = d.clone().downcast::<RwLock<T>>().map_err(|_| {
            zerotrace_core::error::Error::resource_type_mismatch(
                std::any::type_name::<T>(),
                "direct Arc<T>",
                "Arc<RwLock<T>>",
            )
        })?;
        self.metrics.record_world_get(start.elapsed().as_nanos() as u64);
        Ok((result, m.snapshot()))
    }

    fn _get_raw<T: Any + Send + Sync>(
        &self,
        key: ResourceKey,
    ) -> Result<(Arc<T>, ResourceMetaSnapshot)> {
        let start = std::time::Instant::now();
        let map = self.resources.read();
        let (d, m) =
            map.get(&key).ok_or_else(|| zerotrace_core::error::Error::ResourceNotFound {
                type_name: std::any::type_name::<T>(),
            })?;
        let result = d.clone().downcast::<T>().map_err(|_| {
            zerotrace_core::error::Error::resource_type_mismatch(
                std::any::type_name::<T>(),
                "Arc<RwLock<T>>",
                "direct Arc<T>",
            )
        })?;
        self.metrics.record_world_get(start.elapsed().as_nanos() as u64);
        Ok((result, m.snapshot()))
    }

    fn _get_meta_arc<T: Any + Send + Sync>(
        &self,
        key: ResourceKey,
    ) -> Result<(Arc<RwLock<T>>, Arc<ResourceMeta>)> {
        let map = self.resources.read();
        let (d, m) =
            map.get(&key).ok_or_else(|| zerotrace_core::error::Error::ResourceNotFound {
                type_name: std::any::type_name::<T>(),
            })?;
        Ok((
            d.clone().downcast::<RwLock<T>>().map_err(|_| {
                zerotrace_core::error::Error::resource_type_mismatch(
                    std::any::type_name::<T>(),
                    "direct Arc<T>",
                    "Arc<RwLock<T>>",
                )
            })?,
            m.clone(),
        ))
    }

    /// Insert a type-erased resource.  On overwrite, reuses the existing
    /// [`ResourceMeta`] so outstanding handles detect the change.
    pub fn insert_erased(
        &self,
        key: ResourceKey,
        data: Arc<dyn Any + Send + Sync>,
        meta: Arc<ResourceMeta>,
    ) {
        let mut map = self.resources.write();
        let reused_meta = if let Some((_, existing_meta)) = map.get(&key) {
            // Overwrite: bump the existing meta so outstanding ResMut
            // handles see the change, then replace the data.
            let new_tick = meta.added_tick.load(Ordering::Acquire);
            existing_meta.changed_tick.store(new_tick, Ordering::Release);
            existing_meta.added_tick.store(new_tick, Ordering::Release);
            existing_meta.clone()
        } else {
            meta
        };
        map.insert(key, (data, reused_meta));
    }

    pub fn len(&self) -> usize {
        self.resources.read().len()
    }
    pub fn is_empty(&self) -> bool {
        self.resources.read().is_empty()
    }

    /// Drain the command queue without applying any commands.
    /// Used to discard orphaned commands after a system failure,
    /// preventing "ghost" mutations on the next tick.
    pub fn clear_commands(&self) {
        if let Ok((cq, _)) = self.get_raw::<CommandQueue>() {
            cq.buffer.lock().clear();
        }
    }

    pub fn apply_commands(&self) {
        let cmds: Vec<ErasedCommand> = {
            if let Ok((cq, _)) = self.get_raw::<CommandQueue>() {
                cq.buffer.lock().drain(..).collect()
            } else {
                Vec::new()
            }
        };
        for cmd in cmds {
            match cmd {
                ErasedCommand::Insert(key, d) =>
                    self.insert_erased(key, d, Arc::new(ResourceMeta::new(self.next_tick()))),
                ErasedCommand::Remove(key) => {
                    self.resources.write().remove(&key);
                },
            }
        }
    }

    pub fn clear(&self) {
        self.next_tick();
        self.resources.write().clear();
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, PartialEq, Eq)]
    struct C(u64);
    #[test]
    fn test_insert_get() {
        let w = World::new();
        w.insert(C(42));
        let (l, _) = w.get::<C>().unwrap();
        assert_eq!(l.read().0, 42);
    }
    #[test]
    fn test_contains() {
        let w = World::new();
        w.insert(C(1));
        assert!(w.contains::<C>());
    }
    #[test]
    fn test_len() {
        // World::new() registers CommandQueue and LifecycleRegistry
        assert_eq!(World::new().len(), 2);
    }
    #[test]
    fn test_keyed_insert_get() {
        struct KeyA;
        struct KeyB;
        let w = World::new();
        w.insert_keyed::<KeyA, C>(C(1));
        w.insert_keyed::<KeyB, C>(C(2));
        let (a, _) = w.get_keyed::<KeyA, C>().unwrap();
        let (b, _) = w.get_keyed::<KeyB, C>().unwrap();
        assert_eq!(a.read().0, 1);
        assert_eq!(b.read().0, 2);
    }
    #[test]
    fn test_keyed_contains() {
        struct KeyA;
        let w = World::new();
        w.insert_keyed::<KeyA, C>(C(42));
        assert!(w.contains_keyed::<KeyA, C>());
        assert!(!w.contains_keyed::<(), C>());
    }
    #[test]
    fn test_keyed_remove() {
        struct KeyA;
        let w = World::new();
        w.insert_keyed::<KeyA, C>(C(42));
        assert!(w.contains_keyed::<KeyA, C>());
        let removed = w.remove_keyed::<KeyA, C>();
        assert!(removed.is_some());
        assert!(!w.contains_keyed::<KeyA, C>());
    }
}
