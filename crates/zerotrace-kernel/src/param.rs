// SystemParam: Bevy-style DI with SystemContext-driven change detection.
// Uses Arc<RwLock<T>> internally — safe, simple, zero-unsafe in public API.
// is_changed() compares against system's last_run automatically (Bevy pattern).

use crate::{
    error::Result,
    event::Events,
    world::{ResourceMetaSnapshot, SystemContext, Tick, World},
};
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

pub trait SystemParam: Sized + Send + Sync {
    fn fetch(world: &World, ctx: &SystemContext) -> Result<Self>;
}

// ── Res<T> ──────────────────────────────────────────────────────────

pub struct Res<T: Any + Send + Sync> {
    lock: Arc<RwLock<T>>,
    meta: ResourceMetaSnapshot,
    last_run: Tick,
}
impl<T: Any + Send + Sync> Res<T> {
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.lock.read()
    }
    pub fn is_changed(&self) -> bool {
        self.meta.changed_tick > self.last_run
    }
    pub fn is_added(&self) -> bool {
        self.meta.added_tick > self.last_run
    }
}
impl<T: Any + Send + Sync> SystemParam for Res<T> {
    fn fetch(w: &World, c: &SystemContext) -> Result<Self> {
        let (l, m) = w.get::<T>()?;
        Ok(Res {
            lock: l,
            meta: m,
            last_run: c.last_run,
        })
    }
}

// ── ResMut<T> ───────────────────────────────────────────────────────

pub struct ResMut<T: Any + Send + Sync> {
    lock: Arc<RwLock<T>>,
    meta: Arc<crate::world::ResourceMeta>,
    last_run: Tick,
    /// Set to true after the first guard drop that actually mutated the
    /// resource.  Used together with `meta.changed_tick` to implement
    /// `is_changed()`.
    mutated: AtomicBool,
}

/// Guard returned by [`ResMut::write()`].  Defers the `changed_tick`
/// bump until [`Drop`], and only bumps if [`DerefMut`] was actually
/// invoked — a write-guard that is never dereferenced (e.g. held
/// speculatively) won't flag the resource as changed.
pub struct ResMutGuard<'a, T: Any + Send + Sync> {
    guard: RwLockWriteGuard<'a, T>,
    meta: &'a AtomicU64,
    mutated: &'a AtomicBool,
    touched: bool,
}

impl<T: Any + Send + Sync> std::ops::Deref for ResMutGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: Any + Send + Sync> std::ops::DerefMut for ResMutGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.touched = true;
        &mut self.guard
    }
}

impl<T: Any + Send + Sync> Drop for ResMutGuard<'_, T> {
    fn drop(&mut self) {
        // Rust drops fields in declaration order: `guard` (the
        // RwLockWriteGuard) is dropped first, releasing the write lock.
        // Then the remaining fields are dropped (no lock held).
        if self.touched {
            // Only bump once per ResMut per tick, even if multiple guards
            // are created and dropped (sequential scheduler guarantees
            // only one guard is alive at a time).
            if !self.mutated.swap(true, Ordering::AcqRel) {
                self.meta.fetch_add(1, Ordering::Release);
            }
        }
    }
}

impl<T: Any + Send + Sync> ResMut<T> {
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.lock.read()
    }
    /// Acquire a mutable reference to the resource.  Returns a guard that
    /// defers `changed_tick` update to drop time, and only bumps if the
    /// guard was actually used for mutation (i.e. [`DerefMut`] was called).
    ///
    /// This is more precise than the old eager-bump: holding a `write()`
    /// guard without mutating (e.g. in a conditional branch) no longer
    /// flags the resource as changed.
    pub fn write(&self) -> ResMutGuard<'_, T> {
        ResMutGuard {
            guard: self.lock.write(),
            meta: &self.meta.changed_tick,
            mutated: &self.mutated,
            touched: false,
        }
    }
    /// True if the resource was mutated since this system's `last_run`.
    /// Detects both `World::insert()` and in-place `ResMutGuard` mutations.
    pub fn is_changed(&self) -> bool {
        self.mutated.load(Ordering::Acquire) ||
            self.meta.changed_tick.load(Ordering::Acquire) > self.last_run
    }
}
impl<T: Any + Send + Sync> SystemParam for ResMut<T> {
    fn fetch(w: &World, c: &SystemContext) -> Result<Self> {
        let (l, m) = w.get_meta_arc::<T>()?;
        Ok(ResMut {
            lock: l,
            meta: m,
            last_run: c.last_run,
            mutated: AtomicBool::new(false),
        })
    }
}

// ── ResLifecycle ───────────────────────────────────────────────────────

/// Provides access to the [`LifecycleRegistry`](crate::lifecycle::LifecycleRegistry)
/// stored in the [`World`].
pub struct ResLifecycle {
    registry: Arc<crate::lifecycle::LifecycleRegistry>,
}
impl ResLifecycle {
    pub fn registry(&self) -> &Arc<crate::lifecycle::LifecycleRegistry> {
        &self.registry
    }
}
impl SystemParam for ResLifecycle {
    fn fetch(w: &World, _: &SystemContext) -> Result<Self> {
        let (r, _) = w.get_raw::<crate::lifecycle::LifecycleRegistry>()?;
        Ok(ResLifecycle { registry: r })
    }
}

// ── ResKeyed<K, T> ────────────────────────────────────────────────────

/// A keyed resource reference.  Allows multiple instances of the same
/// value type `T` distinguished by a key type `K` to coexist in the
/// [`World`].
///
/// See [`World::insert_keyed`](crate::world::World::insert_keyed).
pub struct ResKeyed<K: Any + Send + Sync, T: Any + Send + Sync> {
    inner: Res<T>,
    _key: std::marker::PhantomData<K>,
}
impl<K: Any + Send + Sync, T: Any + Send + Sync> ResKeyed<K, T> {
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.inner.read()
    }
    pub fn is_changed(&self) -> bool {
        self.inner.is_changed()
    }
    pub fn is_added(&self) -> bool {
        self.inner.is_added()
    }
}
impl<K: Any + Send + Sync, T: Any + Send + Sync> SystemParam for ResKeyed<K, T> {
    fn fetch(w: &World, c: &SystemContext) -> Result<Self> {
        let (l, m) = w.get_keyed::<K, T>()?;
        Ok(ResKeyed {
            inner: Res {
                lock: l,
                meta: m,
                last_run: c.last_run,
            },
            _key: std::marker::PhantomData,
        })
    }
}

/// An optional keyed resource reference.  Returns `None` if the
/// keyed resource is not registered in the World.  Type mismatches
/// are still propagated as errors.
impl<K: Any + Send + Sync, T: Any + Send + Sync> SystemParam for Option<ResKeyed<K, T>> {
    fn fetch(w: &World, c: &SystemContext) -> Result<Self> {
        match ResKeyed::<K, T>::fetch(w, c) {
            Ok(r) => Ok(Some(r)),
            Err(zerotrace_core::error::Error::ResourceNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ── Cfg<T> ──────────────────────────────────────────────────────────

pub struct Cfg<T: Any + Send + Sync> {
    inner: Res<T>,
}
impl<T: Any + Send + Sync> Cfg<T> {
    pub fn snapshot(&self) -> T
    where
        T: Clone,
    {
        self.inner.read().clone()
    }
    pub fn is_changed(&self) -> bool {
        self.inner.is_changed()
    }
}
impl<T: Any + Send + Sync> SystemParam for Cfg<T> {
    fn fetch(w: &World, c: &SystemContext) -> Result<Self> {
        Res::<T>::fetch(w, c).map(|r| Cfg { inner: r })
    }
}

// ── Optional Res / ResMut ──────────────────────────────────────────

/// An optional resource reference.  Returns `None` if the resource is
/// not registered in the World (as opposed to failing with an error).
///
/// Only [`ResourceNotFound`] is swallowed — type mismatches and other
/// errors are still propagated.
///
/// [`ResourceNotFound`]: zerotrace_core::error::Error::ResourceNotFound
///
/// ```ignore
/// fn optional_system(maybe_cfg: Option<Res<FeatureConfig>>) -> Result<()> {
///     if let Some(cfg) = maybe_cfg {
///         // ...
///     }
///     Ok(())
/// }
/// ```
impl<T: Any + Send + Sync> SystemParam for Option<Res<T>> {
    fn fetch(w: &World, c: &SystemContext) -> Result<Self> {
        match Res::<T>::fetch(w, c) {
            Ok(r) => Ok(Some(r)),
            Err(zerotrace_core::error::Error::ResourceNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// An optional mutable resource reference.  Returns `None` if the
/// resource is not registered in the World.  Type mismatches are
/// still propagated as errors.
impl<T: Any + Send + Sync> SystemParam for Option<ResMut<T>> {
    fn fetch(w: &World, c: &SystemContext) -> Result<Self> {
        match ResMut::<T>::fetch(w, c) {
            Ok(r) => Ok(Some(r)),
            Err(zerotrace_core::error::Error::ResourceNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// An optional configuration reference.  Returns `None` if the
/// configuration is not registered in the World.  Type mismatches
/// are still propagated as errors.
impl<T: Any + Send + Sync> SystemParam for Option<Cfg<T>> {
    fn fetch(w: &World, c: &SystemContext) -> Result<Self> {
        match Cfg::<T>::fetch(w, c) {
            Ok(r) => Ok(Some(r)),
            Err(zerotrace_core::error::Error::ResourceNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ── Commands ────────────────────────────────────────────────────────

/// Deferred command buffer for World insert/remove operations.
///
/// `CommandQueue` is stored in the World via [`insert_raw`](World::insert_raw)
/// (no outer `RwLock`), so [`fetch`](SystemParam::fetch) returns this handle
/// with a single lock acquisition.  Mutual exclusion is provided by the
/// inner `Mutex<Vec<ErasedCommand>>` on the queue itself.
///
/// # Shared-state semantics
///
/// Although `Commands` takes `&mut self` in its mutation methods, the
/// underlying [`CommandQueue`] is shared via `Arc` — every system that
/// receives `Commands` as a [`SystemParam`] pushes to the same queue.
/// The `&mut self` prevents accidental aliasing within a single system
/// but does NOT provide cross-system exclusivity.
pub struct Commands {
    queue: Arc<crate::world::CommandQueue>,
}
impl Commands {
    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) {
        self.queue.buffer.lock().push(crate::world::ErasedCommand::Insert(
            crate::world::ResourceKey::of::<T>(),
            Arc::new(RwLock::new(value)),
        ));
    }
    pub fn insert_keyed<K: Any, T: Any + Send + Sync>(&mut self, value: T) {
        self.queue.buffer.lock().push(crate::world::ErasedCommand::Insert(
            crate::world::ResourceKey::keyed::<K, T>(),
            Arc::new(RwLock::new(value)),
        ));
    }
    pub fn remove<T: Any + Send + Sync>(&mut self) {
        self.queue.buffer.lock().push(crate::world::ErasedCommand::Remove(
            crate::world::ResourceKey::of::<T>(),
        ));
    }
    pub fn remove_keyed<K: Any, T: Any + Send + Sync>(&mut self) {
        self.queue.buffer.lock().push(crate::world::ErasedCommand::Remove(
            crate::world::ResourceKey::keyed::<K, T>(),
        ));
    }
}
impl SystemParam for Commands {
    fn fetch(w: &World, _: &SystemContext) -> Result<Self> {
        let (q, _) = w.get_raw::<crate::world::CommandQueue>()?;
        Ok(Commands { queue: q })
    }
}

// ── Events ──────────────────────────────────────────────────────────

/// `Events<T>` is stored in the World via [`insert_raw`](World::insert_raw)
/// because it manages its own synchronisation (double-buffered `Mutex`).
/// This avoids a redundant outer `RwLock` acquisition on every send/drain.
pub struct EventWriter<T: Send + Sync + 'static> {
    events: Arc<Events<T>>,
}
impl<T: Send + Sync + 'static> EventWriter<T> {
    pub fn write(&mut self, e: T) {
        self.events.send(e);
    }
    /// Push multiple events in a single lock acquisition.
    pub fn send_batch(&mut self, events: impl IntoIterator<Item = T>) {
        self.events.send_batch(events);
    }
    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
impl<T: Send + Sync + 'static> SystemParam for EventWriter<T> {
    fn fetch(w: &World, _: &SystemContext) -> Result<Self> {
        let (l, _) = w.get_raw::<Events<T>>()?;
        Ok(EventWriter { events: l })
    }
}

pub struct EventReader<T: Send + Sync + 'static> {
    events: Arc<Events<T>>,
}
impl<T: Send + Sync + 'static> EventReader<T> {
    pub fn drain(&mut self) -> Vec<T> {
        self.events.drain()
    }
    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
impl<T: Send + Sync + 'static> SystemParam for EventReader<T> {
    fn fetch(w: &World, _: &SystemContext) -> Result<Self> {
        let (l, _) = w.get_raw::<Events<T>>()?;
        Ok(EventReader { events: l })
    }
}

// ── Tuples ──────────────────────────────────────────────────────────

macro_rules! tuple_impl { ($($T:ident),+) => { impl<$($T: SystemParam),*> SystemParam for ($($T,)*) { fn fetch(w:&World,c:&SystemContext)->Result<Self>{Ok(($($T::fetch(w,c)?,)*))} } }; }
tuple_impl!(A);
tuple_impl!(A, B);
tuple_impl!(A, B, C);
tuple_impl!(A, B, C, D);
tuple_impl!(A, B, C, D, E);
tuple_impl!(A, B, C, D, E, F);
tuple_impl!(A, B, C, D, E, F, G);
tuple_impl!(A, B, C, D, E, F, G, H);
tuple_impl!(A, B, C, D, E, F, G, H, I);
tuple_impl!(A, B, C, D, E, F, G, H, I, J);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L, M);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);
impl SystemParam for () {
    fn fetch(_: &World, _: &SystemContext) -> Result<Self> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Events;
    #[derive(Debug, PartialEq, Clone)]
    struct DbUrl(String);
    #[derive(Debug, PartialEq, Clone)]
    struct Max(u32);
    #[derive(Debug, PartialEq)]
    struct Cnt(u64);
    #[derive(Debug, Clone, PartialEq)]
    struct Ev(u32);
    fn ctx() -> SystemContext {
        SystemContext::new(2, 1)
    }
    #[test]
    fn test_res() {
        let w = World::new();
        w.insert(DbUrl("x".into()));
        let r: Res<DbUrl> = Res::fetch(&w, &ctx()).unwrap();
        assert_eq!(&r.read().0 as &str, "x");
    }
    #[test]
    fn test_changed() {
        let w = World::new();
        w.insert(Cnt(0));
        let r: Res<Cnt> = Res::fetch(&w, &SystemContext::new(2, 1)).unwrap();
        assert!(r.is_changed());
    }
    #[test]
    fn test_resmut() {
        let w = World::new();
        w.insert(Cnt(0));
        let r: ResMut<Cnt> = ResMut::fetch(&w, &ctx()).unwrap();
        assert_eq!(r.read().0, 0);
    }
    #[test]
    fn test_cmd() {
        let w = World::new();
        let mut c = Commands::fetch(&w, &ctx()).unwrap();
        c.insert(Max(10));
        w.apply_commands();
        assert_eq!(Res::<Max>::fetch(&w, &ctx()).unwrap().read().0, 10);
    }
    #[test]
    fn test_ev() {
        let w = World::new();
        w.insert_raw(Arc::new(Events::<Ev>::new()));
        let mut wt = EventWriter::<Ev>::fetch(&w, &ctx()).unwrap();
        wt.write(Ev(1));
        let mut rd = EventReader::<Ev>::fetch(&w, &ctx()).unwrap();
        assert_eq!(rd.drain(), vec![Ev(1)]);
    }

    #[test]
    fn test_option_res_present() {
        let w = World::new();
        w.insert(Cnt(42));
        let r: Option<Res<Cnt>> = Option::<Res<Cnt>>::fetch(&w, &ctx()).unwrap();
        assert!(r.is_some());
        assert_eq!(r.unwrap().read().0, 42);
    }

    #[test]
    fn test_option_res_absent() {
        let w = World::new();
        let r: Option<Res<Cnt>> = Option::<Res<Cnt>>::fetch(&w, &ctx()).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn test_option_resmut_present() {
        let w = World::new();
        w.insert(Cnt(7));
        let r: Option<ResMut<Cnt>> = Option::<ResMut<Cnt>>::fetch(&w, &ctx()).unwrap();
        assert!(r.is_some());
        r.unwrap().write().0 = 99;
        assert_eq!(w.get::<Cnt>().unwrap().0.read().0, 99);
    }

    #[test]
    fn test_option_resmut_absent() {
        let w = World::new();
        let r: Option<ResMut<Cnt>> = Option::<ResMut<Cnt>>::fetch(&w, &ctx()).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn test_option_cfg_present() {
        let w = World::new();
        w.insert(DbUrl("optional_cfg".into()));
        let c: Option<Cfg<DbUrl>> = Option::<Cfg<DbUrl>>::fetch(&w, &ctx()).unwrap();
        assert!(c.is_some());
        assert_eq!(&c.unwrap().snapshot().0 as &str, "optional_cfg");
    }

    #[test]
    fn test_option_cfg_absent() {
        let w = World::new();
        let c: Option<Cfg<DbUrl>> = Option::<Cfg<DbUrl>>::fetch(&w, &ctx()).unwrap();
        assert!(c.is_none());
    }
}
