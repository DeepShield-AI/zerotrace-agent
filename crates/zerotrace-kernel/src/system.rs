// System, Scheduler — Bevy-style execution with per-system `last_run` tracking.
// Supports dynamic enable/disable, runtime system add/remove, and before/after
// dependency ordering via topological sort.

use crate::{
    error::Result,
    param::SystemParam,
    world::{SystemContext, Tick, World},
};
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    marker::PhantomData,
    pin::Pin,
};
use tracing;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Stage {
    Startup,
    PreUpdate,
    Update,
    PostUpdate,
    Shutdown,
    Custom(&'static str),
}

#[derive(Debug, Clone)]
pub struct SystemMeta {
    pub name: &'static str,
    pub last_run: Tick,
}
impl SystemMeta {
    pub fn new(name: &'static str) -> Self {
        Self { name, last_run: 0 }
    }
}

/// Declares which resource types a system reads and writes.
/// Used by the scheduler for conflict-detection-based parallelism
/// when the `parallel` feature is enabled.
#[derive(Debug, Clone)]
pub struct SystemAccess {
    pub reads: Vec<std::any::TypeId>,
    pub writes: Vec<std::any::TypeId>,
}

impl SystemAccess {
    pub fn empty() -> Self {
        Self {
            reads: vec![],
            writes: vec![],
        }
    }
    /// True if two system accesses conflict.
    pub fn conflicts_with(&self, other: &SystemAccess) -> bool {
        // A conflicts with B if A writes to something B reads or writes,
        // or vice versa.
        for w in &self.writes {
            if other.reads.contains(w) || other.writes.contains(w) {
                return true;
            }
        }
        for w in &other.writes {
            if self.reads.contains(w) {
                return true;
            }
        }
        false
    }
}

// ── Sync System ──────────────────────────────────────────────────────

pub trait System: Send + Sync + 'static {
    fn meta(&self) -> &SystemMeta;
    fn meta_mut(&mut self) -> &mut SystemMeta;
    fn run(&mut self, world: &World, ctx: &SystemContext) -> Result<()>;
    fn run_once(&self) -> bool {
        false
    }
    /// Unique label for dependency ordering.
    fn label(&self) -> Option<&'static str> {
        None
    }
    fn enabled(&self) -> bool {
        true
    }
    fn set_enabled(&mut self, _enabled: bool) {}
    /// Labels this system must run BEFORE.
    fn before_labels(&self) -> &[&'static str] {
        &[]
    }
    /// Labels this system must run AFTER.
    fn after_labels(&self) -> &[&'static str] {
        &[]
    }
    /// Resource access declaration for conflict-detection-based
    /// parallelism.  Default is empty — the system is assumed to
    /// access everything (no parallelism).
    fn access(&self) -> SystemAccess {
        SystemAccess::empty()
    }
}

pub struct FunctionSystem<F, Param> {
    meta: SystemMeta,
    f: F,
    run_once: bool,
    enabled: bool,
    label: Option<&'static str>,
    /// Ordering constraints.  `None` until at least one constraint is added,
    /// avoiding heap allocation for systems without dependencies.
    before: Option<Vec<&'static str>>,
    after: Option<Vec<&'static str>>,
    _marker: PhantomData<Param>,
}

impl<F, Param> FunctionSystem<F, Param> {
    pub fn new(name: &'static str, f: F) -> Self {
        Self {
            meta: SystemMeta::new(name),
            f,
            run_once: false,
            enabled: true,
            label: None,
            before: None,
            after: None,
            _marker: PhantomData,
        }
    }
    pub fn run_once(mut self) -> Self {
        self.run_once = true;
        self
    }
    pub fn label(mut self, l: &'static str) -> Self {
        self.label = Some(l);
        self
    }
    /// Declare that this system must run **before** the system with the
    /// given label.  Allocates the constraint list on first call.
    pub fn before(mut self, l: &'static str) -> Self {
        self.before.get_or_insert_with(Vec::new).push(l);
        self
    }
    /// Declare that this system must run **after** the system with the
    /// given label.  Allocates the constraint list on first call.
    pub fn after(mut self, l: &'static str) -> Self {
        self.after.get_or_insert_with(Vec::new).push(l);
        self
    }
}

impl<F, Param> FunctionSystem<F, Param> {
    fn before_slice(&self) -> &[&'static str] {
        self.before.as_deref().unwrap_or(&[])
    }
    fn after_slice(&self) -> &[&'static str] {
        self.after.as_deref().unwrap_or(&[])
    }
}

impl<F, Param> System for FunctionSystem<F, Param>
where
    F: FnMut(Param) -> Result<()> + Send + Sync + 'static,
    Param: SystemParam + 'static,
{
    fn meta(&self) -> &SystemMeta {
        &self.meta
    }
    fn meta_mut(&mut self) -> &mut SystemMeta {
        &mut self.meta
    }
    fn run(&mut self, w: &World, c: &SystemContext) -> Result<()> {
        (self.f)(Param::fetch(w, c)?)
    }
    fn run_once(&self) -> bool {
        self.run_once
    }
    fn label(&self) -> Option<&'static str> {
        self.label
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, e: bool) {
        self.enabled = e;
    }
    fn before_labels(&self) -> &[&'static str] {
        self.before_slice()
    }
    fn after_labels(&self) -> &[&'static str] {
        self.after_slice()
    }
}

pub trait IntoSystem<Param>: Send + Sync + 'static {
    type System: System;
    fn into_system(self, name: &'static str) -> Self::System;
}

impl<F, Param> IntoSystem<Param> for F
where
    F: FnMut(Param) -> Result<()> + Send + Sync + 'static,
    Param: SystemParam + 'static,
{
    type System = FunctionSystem<F, Param>;
    fn into_system(self, n: &'static str) -> Self::System {
        FunctionSystem::new(n, self)
    }
}

// Identity impl: allow passing pre-configured FunctionSystem directly to add()
impl<F, Param> IntoSystem<Param> for FunctionSystem<F, Param>
where
    F: FnMut(Param) -> Result<()> + Send + Sync + 'static,
    Param: SystemParam + 'static,
{
    type System = FunctionSystem<F, Param>;
    fn into_system(self, _n: &'static str) -> Self::System {
        self
    }
}

// ── Async FunctionSystem (SystemParam-aware) ──────────────────────────

/// An async system built from a closure that receives [`SystemParam`] and
/// returns a [`Future`].  This is the async equivalent of
/// [`FunctionSystem`] — dependencies are injected automatically.
pub struct AsyncFunctionSystem<F, Fut, Param> {
    name: &'static str,
    f: F,
    run_once: bool,
    enabled: bool,
    label: Option<&'static str>,
    before: Option<Vec<&'static str>>,
    after: Option<Vec<&'static str>>,
    _marker: PhantomData<(Fut, Param)>,
}

impl<F, Fut, Param> AsyncFunctionSystem<F, Fut, Param> {
    pub fn new(name: &'static str, f: F) -> Self {
        Self {
            name,
            f,
            run_once: false,
            enabled: true,
            label: None,
            before: None,
            after: None,
            _marker: PhantomData,
        }
    }
    pub fn run_once(mut self) -> Self {
        self.run_once = true;
        self
    }
    pub fn label(mut self, l: &'static str) -> Self {
        self.label = Some(l);
        self
    }
    pub fn before(mut self, l: &'static str) -> Self {
        self.before.get_or_insert_with(Vec::new).push(l);
        self
    }
    pub fn after(mut self, l: &'static str) -> Self {
        self.after.get_or_insert_with(Vec::new).push(l);
        self
    }
}

impl<F, Fut, Param> AsyncSystem for AsyncFunctionSystem<F, Fut, Param>
where
    F: FnMut(Param) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + Sync + 'static,
    Param: SystemParam + 'static,
{
    fn name(&self) -> &'static str {
        self.name
    }
    fn label(&self) -> Option<&'static str> {
        self.label
    }
    fn before_labels(&self) -> &[&'static str] {
        self.before.as_deref().unwrap_or(&[])
    }
    fn after_labels(&self) -> &[&'static str] {
        self.after.as_deref().unwrap_or(&[])
    }
    fn run_once(&self) -> bool {
        self.run_once
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, e: bool) {
        self.enabled = e;
    }
    fn run_async(
        &mut self,
        world: &World,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let param = match Param::fetch(world, &SystemContext::new(world.current_tick(), 0)) {
            Ok(p) => p,
            Err(e) => return Box::pin(async { Err(e) }),
        };
        Box::pin((self.f)(param))
    }
}

/// Trait for converting a closure into an [`AsyncFunctionSystem`].
///
/// This mirrors [`IntoSystem`] for async functions, enabling the same
/// ergonomic registration:
///
/// ```
/// use zerotrace_kernel::param::Res;
/// use zerotrace_kernel::system::{IntoAsyncSystem, Stage, Scheduler};
///
/// struct Config { api_url: String }
///
/// let mut scheduler = Scheduler::new();
/// scheduler.add_async_param(Stage::Update, "fetch",
///     |cfg: Res<Config>| async move {
///         let _url = cfg.read().api_url.clone();
///         Ok(())
///     },
/// );
/// ```
pub trait IntoAsyncSystem<Param, Fut> {
    type System: AsyncSystem;
    fn into_async_system(self, name: &'static str) -> Self::System;
}

impl<F, Fut, Param> IntoAsyncSystem<Param, Fut> for F
where
    F: FnMut(Param) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + Sync + 'static,
    Param: SystemParam + 'static,
{
    type System = AsyncFunctionSystem<F, Fut, Param>;
    fn into_async_system(self, n: &'static str) -> Self::System {
        AsyncFunctionSystem::new(n, self)
    }
}

// ── Async System ─────────────────────────────────────────────────────

/// Trait for systems that execute asynchronously (e.g. I/O, network calls).
/// Async systems are run on a tokio runtime and can spawn their own tasks.
///
/// # Example
///
/// ```
/// use std::pin::Pin;
/// use std::future::Future;
/// use zerotrace_kernel::error::Result;
/// use zerotrace_kernel::world::World;
/// use zerotrace_kernel::AsyncSystem;
///
/// struct MyAsyncSystem { name: &'static str }
///
/// impl AsyncSystem for MyAsyncSystem {
///     fn name(&self) -> &'static str { self.name }
///     fn run_async(
///         &mut self,
///         _world: &World,
///     ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
///         Box::pin(async { Ok(()) })
///     }
/// }
/// ```
pub trait AsyncSystem: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn label(&self) -> Option<&'static str> {
        None
    }
    fn before_labels(&self) -> &[&'static str] {
        &[]
    }
    fn after_labels(&self) -> &[&'static str] {
        &[]
    }
    fn run_once(&self) -> bool {
        false
    }
    fn enabled(&self) -> bool {
        true
    }
    fn set_enabled(&mut self, _enabled: bool) {}
    fn run_async(&mut self, world: &World)
    -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}

// ── Exclusive System ─────────────────────────────────────────────────

pub trait ExclusiveSystem: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn run(&mut self, world: &mut World) -> Result<()>;
    fn run_once(&self) -> bool {
        false
    }
}

pub struct FunctionExclusiveSystem<F> {
    name: &'static str,
    f: F,
    run_once: bool,
}
impl<F> FunctionExclusiveSystem<F> {
    pub fn new(n: &'static str, f: F) -> Self {
        Self {
            name: n,
            f,
            run_once: false,
        }
    }
}
impl<F> ExclusiveSystem for FunctionExclusiveSystem<F>
where
    F: FnMut(&mut World) -> Result<()> + Send + Sync + 'static,
{
    fn name(&self) -> &'static str {
        self.name
    }
    fn run(&mut self, w: &mut World) -> Result<()> {
        (self.f)(w)
    }
    fn run_once(&self) -> bool {
        self.run_once
    }
}

// ── System Entry ─────────────────────────────────────────────────────

struct SyncEntry {
    system: Box<dyn System>,
    /// Original registration index (stable across topological reordering).
    index: usize,
}

struct AsyncEntry {
    system: Box<dyn AsyncSystem>,
    index: usize,
}

// ── Scheduler ────────────────────────────────────────────────────────

/// A group of sync systems in one stage, with ordering cache invalidation.
struct SyncStageGroup {
    stage: Stage,
    entries: Vec<SyncEntry>,
    /// Cached topological order (indices into `entries`).  None when
    /// the stage is dirty and needs re-sorting.
    cached_order: Option<Vec<usize>>,
}

impl SyncStageGroup {
    fn new(stage: Stage) -> Self {
        Self {
            stage,
            entries: Vec::new(),
            cached_order: None,
        }
    }
    fn mark_dirty(&mut self) {
        self.cached_order = None;
    }
}

pub struct Scheduler {
    /// Sync systems grouped by stage with ordering cache.
    stages: Vec<SyncStageGroup>,
    /// Async systems grouped by stage.
    async_stages: Vec<(Stage, Vec<AsyncEntry>)>,
    /// Exclusive systems.
    exclusive: Vec<Box<dyn ExclusiveSystem>>,
    /// Global registration counter for assigning stable indices.
    next_index: usize,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            async_stages: Vec::new(),
            exclusive: Vec::new(),
            next_index: 0,
        }
    }

    // ── Registration ──────────────────────────────────────────────

    /// Add a sync system to a stage.
    pub fn add<Param>(
        &mut self,
        stage: Stage,
        n: &'static str,
        sys: impl IntoSystem<Param>,
    ) -> &mut Self
    where
        Param: SystemParam + 'static,
    {
        let s = sys.into_system(n);
        let idx = self.next_index;
        self.next_index += 1;
        let entry = SyncEntry {
            system: Box::new(s),
            index: idx,
        };
        if let Some(group) = self.stages.iter_mut().find(|g| g.stage == stage) {
            group.entries.push(entry);
            group.mark_dirty();
        } else {
            let mut g = SyncStageGroup::new(stage);
            g.entries.push(entry);
            self.stages.push(g);
        }
        self
    }

    /// Add a startup sync system (runs once in Startup stage).
    pub fn add_startup<Param>(&mut self, n: &'static str, sys: impl IntoSystem<Param>) -> &mut Self
    where
        Param: SystemParam + 'static,
    {
        self.add(Stage::Startup, n, sys)
    }

    /// Add an async system with automatic [`SystemParam`] injection.
    ///
    /// This is the recommended way to register async systems:
    ///
    /// ```
    /// use zerotrace_kernel::param::Res;
    /// use zerotrace_kernel::system::{Stage, Scheduler};
    ///
    /// struct Config { api_url: String }
    ///
    /// let mut scheduler = Scheduler::new();
    /// scheduler.add_async_param(Stage::Update, "fetch",
    ///     |cfg: Res<Config>| async move {
    ///         let _url = cfg.read().api_url.clone();
    ///         Ok(())
    ///     },
    /// );
    /// ```
    pub fn add_async_param<Param, Fut>(
        &mut self,
        stage: Stage,
        n: &'static str,
        sys: impl IntoAsyncSystem<Param, Fut>,
    ) -> &mut Self
    where
        Param: SystemParam + 'static,
        Fut: Future<Output = Result<()>> + Send + Sync + 'static,
    {
        self.add_async(stage, sys.into_async_system(n))
    }

    /// Add an async system to a stage.
    pub fn add_async(&mut self, stage: Stage, system: impl AsyncSystem) -> &mut Self {
        let idx = self.next_index;
        self.next_index += 1;
        let entry = AsyncEntry {
            system: Box::new(system),
            index: idx,
        };
        if let Some(i) = self.async_stages.iter().position(|(s, _)| *s == stage) {
            self.async_stages[i].1.push(entry);
        } else {
            self.async_stages.push((stage, vec![entry]));
        }
        self
    }

    /// Add an exclusive system.
    pub fn add_exclusive(&mut self, sys: impl ExclusiveSystem + 'static) -> &mut Self {
        self.exclusive.push(Box::new(sys));
        self
    }

    // ── Removal / Toggle ──────────────────────────────────────────

    /// Remove a sync system by name. Returns true if removed.
    pub fn remove_by_name(&mut self, name: &str) -> bool {
        for group in self.stages.iter_mut() {
            if let Some(pos) = group.entries.iter().position(|e| e.system.meta().name == name) {
                group.entries.remove(pos);
                group.mark_dirty();
                return true;
            }
        }
        false
    }

    /// Remove an async system by name. Returns true if removed.
    pub fn remove_async_by_name(&mut self, name: &str) -> bool {
        for (_, entries) in self.async_stages.iter_mut() {
            if let Some(pos) = entries.iter().position(|e| e.system.name() == name) {
                entries.remove(pos);
                return true;
            }
        }
        false
    }

    /// Enable or disable a sync system by name.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> bool {
        for group in self.stages.iter_mut() {
            for e in group.entries.iter_mut() {
                if e.system.meta().name == name {
                    e.system.set_enabled(enabled);
                    return true;
                }
            }
        }
        false
    }

    /// Enable or disable an async system by name.
    pub fn set_async_enabled(&mut self, name: &str, enabled: bool) -> bool {
        for (_, entries) in self.async_stages.iter_mut() {
            for e in entries.iter_mut() {
                if e.system.name() == name {
                    e.system.set_enabled(enabled);
                    return true;
                }
            }
        }
        false
    }

    // ── Topological sort for before/after ordering ─────────────────

    /// Resolve `before`/`after` labels within a stage, sorting systems
    /// into topological order. Systems without constraints keep their
    /// registration order relative to each other.
    ///
    /// Results are cached in `group.cached_order`.  Subsequent calls are
    /// a no-op until the stage is marked dirty (systems added/removed).
    fn resolve_ordering(group: &mut SyncStageGroup) -> Result<()> {
        // Cache hit: entries are already in topological order from the
        // last sort.  No work needed — just return.
        if group.cached_order.is_some() {
            return Ok(());
        }

        let n = group.entries.len();
        if n <= 1 {
            group.cached_order = Some((0..n).collect());
            return Ok(());
        }

        let entries = &group.entries;

        // Build label → position (within this stage) map
        let mut label_to_pos: HashMap<&'static str, usize> = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            if let Some(lbl) = entry.system.label() {
                label_to_pos.insert(lbl, i);
            }
        }

        // Build adjacency + in-degree using local positions
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut in_deg: Vec<usize> = vec![0; n];

        for (i, entry) in entries.iter().enumerate() {
            // "I must run BEFORE these labels" → edge i → target
            for &lbl in entry.system.before_labels() {
                if let Some(&target) = label_to_pos.get(lbl) {
                    adj[i].push(target);
                    in_deg[target] += 1;
                } else {
                    tracing::warn!(
                        "system [{}] declares before(\"{}\") but no system in stage has that label",
                        entry.system.meta().name,
                        lbl
                    );
                }
            }
            // "I must run AFTER these labels" → edge source → i
            for &lbl in entry.system.after_labels() {
                if let Some(&source) = label_to_pos.get(lbl) {
                    adj[source].push(i);
                    in_deg[i] += 1;
                } else {
                    tracing::warn!(
                        "system [{}] declares after(\"{}\") but no system in stage has that label",
                        entry.system.meta().name,
                        lbl
                    );
                }
            }
        }

        // Kahn's algorithm for topological sort
        let mut queue: VecDeque<usize> = VecDeque::new();
        for (i, deg) in in_deg.iter().enumerate() {
            if *deg == 0 {
                queue.push_back(i);
            }
        }

        let mut order: Vec<usize> = Vec::with_capacity(n);
        while let Some(u) = queue.pop_front() {
            order.push(u);
            for &v in &adj[u] {
                in_deg[v] = in_deg[v].saturating_sub(1);
                if in_deg[v] == 0 {
                    queue.push_back(v);
                }
            }
        }

        if order.len() != n {
            // Cycle detected — fall back to registration order (no reordering)
            let order_set: std::collections::HashSet<usize> = order.iter().copied().collect();
            let unresolved: Vec<&str> = entries
                .iter()
                .enumerate()
                .filter(|(i, _)| !order_set.contains(i))
                .filter_map(|(_, e)| e.system.label())
                .collect();
            tracing::error!(
                "cycle detected in system ordering (stage has {} systems, sorted {}): labels involved: {:?}. Falling back to registration order.",
                n,
                order.len(),
                unresolved
            );
            // Cache registration order on cycle to avoid repeated Kahn runs
            group.cached_order = Some((0..n).collect());
            return Ok(());
        }

        // Cache the computed topological order
        group.cached_order = Some(order.clone());

        // Build rank map: global entry index → topological rank
        let rank: HashMap<usize, usize> = order
            .iter()
            .enumerate()
            .map(|(topo_rank, &local_pos)| (entries[local_pos].index, topo_rank))
            .collect();
        group.entries.sort_by_key(|e| rank.get(&e.index).copied().unwrap_or(usize::MAX));
        Ok(())
    }

    // ── Run (sync) ────────────────────────────────────────────────

    /// Execute all sync systems in stage order. Each stage's systems
    /// are topologically sorted by `before`/`after` constraints, then
    /// executed. Topological order is cached and only recomputed when
    /// systems are added or removed.
    ///
    /// On error, systems that already completed in prior stages are
    /// preserved; only the failing system and any unexecuted systems in
    /// the same stage are removed.  The scheduler remains usable.
    pub fn run(&mut self, world: &mut World) -> Result<()> {
        let this_run = world.current_tick();

        // 1. Exclusive systems first.
        // Drain into a local Vec to preserve state on error.
        let ex_drained: Vec<Box<dyn ExclusiveSystem>> = self.exclusive.drain(..).collect();
        let mut ex_keep: Vec<Box<dyn ExclusiveSystem>> = Vec::with_capacity(ex_drained.len());
        for mut s in ex_drained {
            if let Err(e) = s.run(world) {
                world.clear_commands();
                self.exclusive = ex_keep;
                return Err(e);
            }
            world.apply_commands();
            if !s.run_once() {
                ex_keep.push(s);
            }
        }
        self.exclusive = ex_keep;

        // 2. Sync systems, stage by stage.
        // On error, remaining unvisited groups are preserved.
        let mut drained: Vec<SyncStageGroup> = self.stages.drain(..).collect();
        let mut new_stages: Vec<SyncStageGroup> = Vec::with_capacity(drained.len());
        let total_groups = drained.len();
        let mut group_idx = 0;
        let mut err: Option<crate::error::Error> = None;
        while group_idx < total_groups {
            let mut group = &mut drained[group_idx];
            group_idx += 1;
            let original_count = group.entries.len();

            // Resolve ordering constraints (uses cache if clean)
            Self::resolve_ordering(&mut group)?;

            let oneshot = group.stage == Stage::Startup || group.stage == Stage::Shutdown;
            // Record run_once status and entry indices BEFORE consuming the vec.
            let pre_info: Vec<(usize, bool)> =
                group.entries.iter().map(|e| (e.index, e.system.run_once())).collect();
            let mut keep: Vec<SyncEntry> = Vec::with_capacity(original_count);

            let entries: Vec<SyncEntry> = std::mem::take(&mut group.entries);
            for entry in entries.into_iter() {
                if !entry.system.enabled() {
                    keep.push(entry);
                    continue;
                }

                let lr = entry.system.meta().last_run;
                let mut sys = entry.system;
                if let Err(e) = sys.run(world, &SystemContext::new(this_run, lr)) {
                    world.clear_commands();
                    err = Some(e);
                    // Push already-executed systems (keep) + remaining
                    // entries so the scheduler survives the error.
                    break;
                }
                sys.meta_mut().last_run = this_run;
                world.apply_commands();

                if !oneshot && !sys.run_once() {
                    keep.push(SyncEntry {
                        system: sys,
                        index: entry.index,
                    });
                }
            }

            if err.is_some() {
                // Save the partially-completed group (already-executed
                // systems + unexecuted ones).  The cached order must be
                // invalidated because some systems may have changed state.
                if !keep.is_empty() {
                    new_stages.push(SyncStageGroup {
                        stage: group.stage.clone(),
                        entries: keep,
                        cached_order: None,
                    });
                }
                // Append remaining unvisited groups
                if group_idx < total_groups {
                    new_stages.extend(drained.drain(group_idx..));
                }
                self.stages = new_stages;
                return Err(err.unwrap());
            }

            if !keep.is_empty() {
                let kept_indices: std::collections::HashSet<usize> =
                    keep.iter().map(|e| e.index).collect();
                let all_removals_were_run_once =
                    pre_info.iter().all(|(idx, run_once)| kept_indices.contains(idx) || *run_once);
                let cached_order = if keep.len() == original_count || all_removals_were_run_once {
                    group.cached_order.clone()
                } else {
                    None
                };
                new_stages.push(SyncStageGroup {
                    stage: group.stage.clone(),
                    entries: keep,
                    cached_order,
                });
            }
        }
        self.stages = new_stages;
        Ok(())
    }

    /// Execute sync + async systems. Async systems run on the provided
    /// tokio runtime handle. Exclusive systems run first, then sync
    /// systems stage-by-stage, then async systems stage-by-stage.
    pub async fn run_async(
        &mut self,
        world: &mut World,
        handle: &tokio::runtime::Handle,
    ) -> Result<()> {
        // 1. Run exclusive + sync systems first
        if let Err(e) = self.run(world) {
            return Err(e);
        }

        // 2. Run async systems stage by stage.
        // Drain into local Vec to preserve state on error.
        let mut async_drained: Vec<(Stage, Vec<AsyncEntry>)> =
            self.async_stages.drain(..).collect();
        let _guard = handle.enter();
        let mut new_async_stages: Vec<(Stage, Vec<AsyncEntry>)> =
            Vec::with_capacity(async_drained.len());
        let total_async = async_drained.len();
        let mut async_idx = 0;
        while async_idx < total_async {
            let (ref stage, ref mut entries) = async_drained[async_idx];
            let stage = stage.clone();
            let entries: Vec<AsyncEntry> = std::mem::take(entries);
            async_idx += 1;
            let oneshot = stage == Stage::Startup || stage == Stage::Shutdown;
            let mut keep: Vec<AsyncEntry> = Vec::with_capacity(entries.len());

            for entry in entries.into_iter() {
                if !entry.system.enabled() {
                    keep.push(entry);
                    continue;
                }

                let mut sys = entry.system;
                if let Err(e) = sys.run_async(world).await {
                    world.clear_commands();
                    // Preserve already-executed systems + remaining stages
                    if !keep.is_empty() {
                        new_async_stages.push((stage, keep));
                    }
                    if async_idx < total_async {
                        new_async_stages.extend(async_drained.drain(async_idx..));
                    }
                    self.async_stages = new_async_stages;
                    return Err(e);
                }

                if !oneshot && !sys.run_once() {
                    keep.push(AsyncEntry {
                        system: sys,
                        index: entry.index,
                    });
                }
            }
            if !keep.is_empty() {
                new_async_stages.push((stage, keep));
            }
        }
        self.async_stages = new_async_stages;
        Ok(())
    }

    /// Execute sync systems with intra-stage parallelism.
    ///
    /// Within each stage, systems are grouped into conflict-free batches
    /// based on their [`System::access`] declarations.  Batches run
    /// sequentially in topological order; within a batch, systems run
    /// concurrently via [`std::thread::scope`].
    ///
    /// Systems that do not declare `access()` (empty reads + writes) are
    /// treated as if they access everything and each get a singleton batch
    /// (sequential execution).
    ///
    /// Exclusive systems still run first, sequentially.
    ///
    /// # Safety
    ///
    /// Uses raw pointers to obtain `&mut` references to non-overlapping
    /// vector slots within a batch.  Each batch index is unique, so no
    /// two threads alias the same `SyncEntry`.  `std::thread::scope`
    /// guarantees all threads join before the batch completes.
    pub fn run_parallel(&mut self, world: &mut World) -> Result<()> {
        let this_run = world.current_tick();

        // 1. Exclusive systems first (sequential).
        let ex_drained: Vec<Box<dyn ExclusiveSystem>> = self.exclusive.drain(..).collect();
        let mut ex_keep: Vec<Box<dyn ExclusiveSystem>> = Vec::with_capacity(ex_drained.len());
        for mut s in ex_drained {
            if let Err(e) = s.run(world) {
                world.clear_commands();
                self.exclusive = ex_keep;
                return Err(e);
            }
            world.apply_commands();
            if !s.run_once() {
                ex_keep.push(s);
            }
        }
        self.exclusive = ex_keep;

        // 2. Sync systems, stage by stage with intra-stage parallelism
        let mut new_stages: Vec<SyncStageGroup> = Vec::new();
        // Drain into a local Vec first so we can re-assemble on error.
        let mut drained: Vec<SyncStageGroup> = self.stages.drain(..).collect();
        let total_groups_par = drained.len();
        let mut gp_idx = 0;
        let mut err: Option<crate::error::Error> = None;
        while gp_idx < total_groups_par {
            let mut group = &mut drained[gp_idx];
            gp_idx += 1;
            let original_count = group.entries.len();
            Self::resolve_ordering(&mut group)?;
            let oneshot = group.stage == Stage::Startup || group.stage == Stage::Shutdown;

            // Collect access declarations and build conflict-free batches
            let accesses: Vec<SystemAccess> =
                group.entries.iter().map(|e| e.system.access()).collect();
            let batches = Self::greedy_batch(&accesses);

            // Record run_once status BEFORE execution, using global indices
            // (same pattern as `run()` above — see comment there).
            let run_once_info: Vec<(usize, bool)> =
                group.entries.iter().map(|e| (e.index, e.system.run_once())).collect();

            // Shared error slot for parallel batches.
            let thread_err: std::sync::Mutex<Option<crate::error::Error>> =
                std::sync::Mutex::new(None);

            // Execute batch by batch
            'batches: for batch in &batches {
                let batch_has_access_decl = batch.iter().any(|&idx| {
                    let a = &accesses[idx];
                    !a.reads.is_empty() || !a.writes.is_empty()
                });

                if batch.len() == 1 || !batch_has_access_decl {
                    // ── Sequential execution ─────────────────────────
                    for &idx in batch {
                        let entry = &mut group.entries[idx];
                        if !entry.system.enabled() {
                            continue;
                        }
                        let lr = entry.system.meta().last_run;
                        if let Err(e) = entry.system.run(world, &SystemContext::new(this_run, lr)) {
                            world.clear_commands();
                            err = Some(e);
                            break 'batches;
                        }
                        entry.system.meta_mut().last_run = this_run;
                        world.apply_commands();
                    }
                } else {
                    // ── Parallel execution ───────────────────────────
                    // Batch indices are unique → no aliasing.
                    // Strategy:
                    //   1. Move Box<dyn System> out of each SyncEntry
                    //      in the batch (replace with Placeholder).
                    //   2. Move system ownership into scoped threads.
                    //   3. Join threads, collect (index, system) pairs.
                    //   4. Reinsert systems into their entries.
                    //
                    // Box<dyn System> is Send (System: Send + Sync),
                    // so it crosses thread boundaries safely.
                    struct Placeholder(SystemMeta);
                    impl System for Placeholder {
                        fn meta(&self) -> &SystemMeta {
                            &self.0
                        }
                        fn meta_mut(&mut self) -> &mut SystemMeta {
                            &mut self.0
                        }
                        fn run(&mut self, _: &World, _: &SystemContext) -> Result<()> {
                            // Placeholder should never be executed — it
                            // only exists to temporarily hold a slot while
                            // the real system runs in a parallel thread.
                            // If this fires, a batch index was duplicated
                            // (greedy_batch bug).  Return an error rather
                            // than silently succeeding.
                            Err(zerotrace_core::error::Error::Other(
                                "BUG: Placeholder system executed — duplicate index in run_parallel batch".into(),
                            ))
                        }
                        fn enabled(&self) -> bool {
                            false
                        }
                    }

                    let n = batch.len();
                    // Extract: (index, enabled_flag, last_run) +
                    // the owned Box<dyn System> goes into a Vec.
                    let mut sys_slots: Vec<Box<dyn System>> = Vec::with_capacity(n);
                    let mut meta_slots: Vec<(usize, bool, Tick)> = Vec::with_capacity(n);

                    for &idx in batch {
                        let enabled = group.entries[idx].system.enabled();
                        let lr = group.entries[idx].system.meta().last_run;
                        let placeholder = Placeholder(SystemMeta::new("_placeholder"));
                        let sys = std::mem::replace(
                            &mut group.entries[idx].system,
                            Box::new(placeholder),
                        );
                        sys_slots.push(sys);
                        meta_slots.push((idx, enabled, lr));
                    }

                    // Spawn one thread per system (ownership moves in).
                    // Join inside scope; collect (index, system) pairs.
                    let mut results: Vec<(usize, Box<dyn System>)> = Vec::with_capacity(n);
                    std::thread::scope(|s| {
                        let world_ref: &World = &*world;
                        let err_slot = &thread_err;
                        let mut handles = Vec::with_capacity(n);
                        for i in 0..n {
                            let placeholder = Placeholder(SystemMeta::new("_ph"));
                            let mut sys =
                                std::mem::replace(&mut sys_slots[i], Box::new(placeholder));
                            let (idx, enabled, lr) = meta_slots[i];
                            handles.push(s.spawn(move || {
                                if enabled {
                                    match sys.run(world_ref, &SystemContext::new(this_run, lr)) {
                                        Ok(()) => {
                                            sys.meta_mut().last_run = this_run;
                                        },
                                        Err(e) => {
                                            let mut guard = err_slot.lock().unwrap();
                                            if guard.is_none() {
                                                *guard = Some(e);
                                            }
                                        },
                                    }
                                }
                                (idx, sys)
                            }));
                        }
                        for h in handles {
                            results.push(h.join().unwrap());
                        }
                    });

                    // Reinsert systems into their entries
                    for (idx, sys) in results {
                        group.entries[idx].system = sys;
                    }

                    // Propagate first thread error.
                    // Clear commands from the failed parallel batch —
                    // a single thread failure means we discard all
                    // deferred mutations from this batch.
                    if let Some(e) = thread_err.lock().unwrap().take() {
                        world.clear_commands();
                        err = Some(e);
                        break 'batches;
                    }

                    world.apply_commands();
                }
            }

            // ── Rebuild keep list (filter out run_once + disabled) ──
            let mut keep: Vec<SyncEntry> = Vec::with_capacity(original_count);
            let par_entries: Vec<SyncEntry> = std::mem::take(&mut group.entries);
            for (i, entry) in par_entries.into_iter().enumerate() {
                // Keep disabled systems (they may be re-enabled later) and
                // enabled non-oneshot non-run_once systems.
                if !entry.system.enabled() || (!oneshot && !run_once_info[i].1) {
                    keep.push(entry);
                }
            }

            if err.is_some() {
                // Preserve the partial group (already-executed + remaining
                // systems).  Invalidate cached order because some systems
                // may have mutated state.
                if !keep.is_empty() {
                    new_stages.push(SyncStageGroup {
                        stage: group.stage.clone(),
                        entries: keep,
                        cached_order: None,
                    });
                }
                // Append remaining unvisited groups
                if gp_idx < total_groups_par {
                    new_stages.extend(drained.drain(gp_idx..));
                }
                self.stages = new_stages;
                return Err(err.unwrap());
            }

            if !keep.is_empty() {
                let kept_indices: std::collections::HashSet<usize> =
                    keep.iter().map(|e| e.index).collect();
                let all_removals_were_run_once =
                    run_once_info.iter().all(|(idx, ro)| kept_indices.contains(idx) || *ro);
                let cached_order = if keep.len() == original_count || all_removals_were_run_once {
                    group.cached_order.clone()
                } else {
                    None
                };
                new_stages.push(SyncStageGroup {
                    stage: group.stage.clone(),
                    entries: keep,
                    cached_order,
                });
            }
        }
        self.stages = new_stages;
        Ok(())
    }

    // ── Queries ───────────────────────────────────────────────────

    pub fn sync_count(&self) -> usize {
        self.stages.iter().map(|g| g.entries.len()).sum::<usize>()
    }
    pub fn async_count(&self) -> usize {
        self.async_stages.iter().map(|(_, e)| e.len()).sum::<usize>()
    }
    pub fn exclusive_count(&self) -> usize {
        self.exclusive.len()
    }
    pub fn system_count(&self) -> usize {
        self.sync_count() + self.async_count() + self.exclusive_count()
    }

    pub fn clear(&mut self) {
        self.stages.clear();
        self.async_stages.clear();
        self.exclusive.clear();
        self.next_index = 0;
    }

    /// Partition systems into conflict-free batches using a greedy
    /// algorithm.  Systems with empty access (no declaration) each get
    /// their own singleton batch.
    ///
    /// Public for use by external tools that need to inspect scheduling.
    pub fn greedy_batch(accesses: &[SystemAccess]) -> Vec<Vec<usize>> {
        let mut batches: Vec<Vec<usize>> = Vec::new();
        let mut batch_accesses: Vec<SystemAccess> = Vec::new();

        for (i, acc) in accesses.iter().enumerate() {
            // Empty access = "touches everything" → singleton batch
            if acc.reads.is_empty() && acc.writes.is_empty() {
                batches.push(vec![i]);
                continue;
            }
            let mut placed = false;
            for (b_idx, batch_acc) in batch_accesses.iter_mut().enumerate() {
                if !acc.conflicts_with(batch_acc) {
                    batches[b_idx].push(i);
                    for r in &acc.reads {
                        if !batch_acc.reads.contains(r) {
                            batch_acc.reads.push(*r);
                        }
                    }
                    for w in &acc.writes {
                        if !batch_acc.writes.contains(w) {
                            batch_acc.writes.push(*w);
                        }
                    }
                    placed = true;
                    break;
                }
            }
            if !placed {
                batches.push(vec![i]);
                batch_accesses.push(acc.clone());
            }
        }
        batches
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::Error,
        event::Events,
        param::{Commands, Res, ResMut},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Debug, PartialEq)]
    struct Counter(u64);
    #[derive(Debug, Clone, PartialEq)]
    struct Ev(u32);

    #[test]
    fn test_single_system() {
        let mut w = World::new();
        w.insert(Counter(5));
        let mut s = Scheduler::new();
        s.add(Stage::Update, "inc", |c: Res<Counter>| -> Result<()> {
            assert_eq!(*c.read(), Counter(5));
            Ok(())
        });
        s.run(&mut w).unwrap();
    }

    #[test]
    fn test_change_detection() {
        let mut w = World::new();
        w.insert(Counter(1));
        let mut s = Scheduler::new();
        s.add(Stage::Update, "cd", |c: Res<Counter>| -> Result<()> {
            assert!(c.is_changed());
            Ok(())
        });
        s.run(&mut w).unwrap();
    }

    #[test]
    fn test_stages() {
        let mut w = World::new();
        let log = Arc::new(AtomicUsize::new(0));
        w.insert(log.clone());
        let mut s = Scheduler::new();
        s.add(
            Stage::Startup,
            "s1",
            |l: Res<Arc<AtomicUsize>>| -> Result<()> {
                l.read().store(1, Ordering::SeqCst);
                Ok(())
            },
        );
        s.add(
            Stage::Update,
            "s2",
            |l: Res<Arc<AtomicUsize>>| -> Result<()> {
                l.read().store(2, Ordering::SeqCst);
                Ok(())
            },
        );
        s.run(&mut w).unwrap();
        assert_eq!(log.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_startup_once() {
        let mut w = World::new();
        let c = Arc::new(AtomicUsize::new(0));
        w.insert(c.clone());
        let mut s = Scheduler::new();
        s.add_startup("o", |c: Res<Arc<AtomicUsize>>| -> Result<()> {
            c.read().fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        s.run(&mut w).unwrap();
        s.run(&mut w).unwrap();
        assert_eq!(c.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_commands() {
        #[derive(Debug, PartialEq)]
        struct Flag(u32);
        let mut w = World::new();
        let mut s = Scheduler::new();
        s.add(Stage::Update, "ins", |mut cmd: Commands| -> Result<()> {
            cmd.insert(Flag(42));
            Ok(())
        });
        s.run(&mut w).unwrap();
        let (l, _) = w.get::<Flag>().unwrap();
        assert_eq!(l.read().0, 42);
    }

    #[test]
    fn test_exclusive() {
        let mut w = World::new();
        let mut s = Scheduler::new();
        s.add_exclusive(FunctionExclusiveSystem::new(
            "s",
            |w: &mut World| -> Result<()> {
                w.insert(Counter(99));
                Ok(())
            },
        ));
        s.run(&mut w).unwrap();
        let (l, _) = w.get::<Counter>().unwrap();
        assert_eq!(l.read().0, 99);
    }

    #[test]
    fn test_enable_disable() {
        let mut w = World::new();
        w.insert(Counter(0));
        let mut s = Scheduler::new();
        s.add(Stage::Update, "inc", |c: ResMut<Counter>| -> Result<()> {
            c.write().0 += 1;
            Ok(())
        });
        s.run(&mut w).unwrap();
        assert_eq!(w.get::<Counter>().unwrap().0.read().0, 1);
        assert!(s.set_enabled("inc", false));
        s.run(&mut w).unwrap();
        assert_eq!(w.get::<Counter>().unwrap().0.read().0, 1);
        assert!(s.set_enabled("inc", true));
        s.run(&mut w).unwrap();
        assert_eq!(w.get::<Counter>().unwrap().0.read().0, 2);
    }

    #[test]
    fn test_remove_by_name() {
        let w = World::new();
        w.insert(Counter(0));
        let mut s = Scheduler::new();
        s.add(Stage::Update, "a", |_: ()| -> Result<()> { Ok(()) });
        s.add(Stage::Update, "b", |_: ()| -> Result<()> { Ok(()) });
        assert_eq!(s.sync_count(), 2);
        assert!(s.remove_by_name("a"));
        assert_eq!(s.sync_count(), 1);
        assert!(!s.remove_by_name("nonexistent"));
    }

    // ── Ordering tests ─────────────────────────────────────────

    #[test]
    fn test_before_ordering() {
        #[derive(Debug, PartialEq)]
        struct Seq(Vec<&'static str>);
        let mut w = World::new();
        w.insert(Seq(Vec::new()));
        let mut s = Scheduler::new();
        s.add(Stage::Update, "first", |sq: ResMut<Seq>| -> Result<()> {
            sq.write().0.push("first");
            Ok(())
        });
        // second must run BEFORE third
        s.add(
            Stage::Update,
            "second",
            FunctionSystem::new("second", |sq: ResMut<Seq>| -> Result<()> {
                sq.write().0.push("second");
                Ok(())
            })
            .label("second_lbl")
            .before("third_lbl"),
        );
        s.add(
            Stage::Update,
            "third",
            FunctionSystem::new("third", |sq: ResMut<Seq>| -> Result<()> {
                sq.write().0.push("third");
                Ok(())
            })
            .label("third_lbl"),
        );
        s.run(&mut w).unwrap();
        let (seq, _) = w.get::<Seq>().unwrap();
        let v = &seq.read().0;
        let pos2 = v.iter().position(|&x| x == "second").unwrap();
        let pos3 = v.iter().position(|&x| x == "third").unwrap();
        assert!(pos2 < pos3, "second must precede third, got: {:?}", v);
    }

    #[test]
    fn test_after_ordering() {
        #[derive(Debug, PartialEq)]
        struct Seq(Vec<&'static str>);
        let mut w = World::new();
        w.insert(Seq(Vec::new()));
        let mut s = Scheduler::new();
        s.add(
            Stage::Update,
            "a",
            FunctionSystem::new("a", |sq: ResMut<Seq>| -> Result<()> {
                sq.write().0.push("a");
                Ok(())
            })
            .label("a_lbl"),
        );
        s.add(
            Stage::Update,
            "b",
            FunctionSystem::new("b", |sq: ResMut<Seq>| -> Result<()> {
                sq.write().0.push("b");
                Ok(())
            })
            .label("b_lbl")
            .after("a_lbl"),
        );
        s.run(&mut w).unwrap();
        let (seq, _) = w.get::<Seq>().unwrap();
        let v = &seq.read().0;
        let pos_a = v.iter().position(|&x| x == "a").unwrap();
        let pos_b = v.iter().position(|&x| x == "b").unwrap();
        assert!(pos_a < pos_b, "a must precede b, got: {:?}", v);
    }

    #[test]
    fn test_complex_chain() {
        #[derive(Debug, PartialEq)]
        struct Seq(Vec<&'static str>);
        let mut w = World::new();
        w.insert(Seq(Vec::new()));
        let mut s = Scheduler::new();
        // a → b → c → d  chain using before
        s.add(
            Stage::Update,
            "a",
            FunctionSystem::new("a", |sq: ResMut<Seq>| -> Result<()> {
                sq.write().0.push("a");
                Ok(())
            })
            .label("a")
            .before("b"),
        );
        s.add(
            Stage::Update,
            "b",
            FunctionSystem::new("b", |sq: ResMut<Seq>| -> Result<()> {
                sq.write().0.push("b");
                Ok(())
            })
            .label("b")
            .before("c"),
        );
        s.add(
            Stage::Update,
            "c",
            FunctionSystem::new("c", |sq: ResMut<Seq>| -> Result<()> {
                sq.write().0.push("c");
                Ok(())
            })
            .label("c")
            .before("d"),
        );
        s.add(
            Stage::Update,
            "d",
            FunctionSystem::new("d", |sq: ResMut<Seq>| -> Result<()> {
                sq.write().0.push("d");
                Ok(())
            })
            .label("d"),
        );
        s.run(&mut w).unwrap();
        let (seq, _) = w.get::<Seq>().unwrap();
        assert_eq!(&seq.read().0, &vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn test_cycle_fallback() {
        let mut w = World::new();
        w.insert(Counter(0));
        let mut s = Scheduler::new();
        // a before b, b before a = cycle
        s.add(
            Stage::Update,
            "a",
            FunctionSystem::new("a", |c: Res<Counter>| -> Result<()> {
                let _ = c.read().0;
                Ok(())
            })
            .label("a")
            .before("b"),
        );
        s.add(
            Stage::Update,
            "b",
            FunctionSystem::new("b", |c: Res<Counter>| -> Result<()> {
                let _ = c.read().0;
                Ok(())
            })
            .label("b")
            .before("a"),
        );
        // Cycle should not panic — falls back to registration order
        s.run(&mut w).unwrap();
    }

    #[test]
    fn test_error_recovery_scheduler_survives_error() {
        // Regression: scheduler::run() used self.stages.drain(..) directly
        // in a for loop. On system error, the `?` operator would return
        // early, leaving self.stages in a partially-drained state (broken
        // invariant). The fix drains into a local Vec first, so self.stages
        // is always in a valid state after an error.
        let mut w = World::new();
        w.insert(Counter(0));

        let mut s = Scheduler::new();
        s.add(Stage::Update, "fail", |_: ()| -> Result<()> {
            Err(Error::Other("fail".into()))
        });

        let result = s.run(&mut w);
        assert!(result.is_err());

        // After fix: the scheduler is in a valid state — we can add new
        // systems and run again without panic or broken invariants.
        s.add(Stage::Update, "recovery", |_: ()| -> Result<()> { Ok(()) });
        let result2 = s.run(&mut w);
        assert!(
            result2.is_ok(),
            "scheduler should be usable after error recovery"
        );
    }

    #[test]
    fn test_error_recovery_preserves_completed_stages() {
        // Systems in stages that completed before the failing stage
        // are preserved.
        let mut w = World::new();
        w.insert(Counter(0));

        let mut s = Scheduler::new();
        // PreUpdate: succeeds and completes
        s.add(Stage::PreUpdate, "pre", |_: ()| -> Result<()> { Ok(()) });
        // Shutdown: fails (runs after PreUpdate)
        s.add(Stage::Shutdown, "shut_fail", |_: ()| -> Result<()> {
            Err(Error::Other("shutdown error".into()))
        });

        let result = s.run(&mut w);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("shutdown error"));

        // PreUpdate ran successfully — the scheduler is valid.
        // We can add new systems and run.
        s.add(Stage::Update, "after", |_: ()| -> Result<()> { Ok(()) });
        let result2 = s.run(&mut w);
        assert!(result2.is_ok());
    }

    // ── Async system tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_async_system_runs() {
        struct MyAsync {
            val: Arc<AtomicUsize>,
        }
        impl AsyncSystem for MyAsync {
            fn name(&self) -> &'static str {
                "async_test"
            }
            fn run_async(
                &mut self,
                _world: &World,
            ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
                let v = self.val.clone();
                Box::pin(async move {
                    v.store(42, Ordering::SeqCst);
                    Ok(())
                })
            }
        }

        let val = Arc::new(AtomicUsize::new(0));
        let mut w = World::new();
        let mut s = Scheduler::new();
        s.add_async(Stage::Update, MyAsync { val: val.clone() });
        let handle = tokio::runtime::Handle::current();
        s.run_async(&mut w, &handle).await.unwrap();
        assert_eq!(val.load(Ordering::SeqCst), 42);
    }

    #[tokio::test]
    async fn test_async_system_once() {
        struct OnceAsync {
            count: Arc<AtomicUsize>,
        }
        impl AsyncSystem for OnceAsync {
            fn name(&self) -> &'static str {
                "once_async"
            }
            fn run_once(&self) -> bool {
                true
            }
            fn run_async(
                &mut self,
                _world: &World,
            ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
                let v = self.count.clone();
                Box::pin(async move {
                    v.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }
        }

        let count = Arc::new(AtomicUsize::new(0));
        let mut w = World::new();
        let mut s = Scheduler::new();
        s.add_async(
            Stage::Update,
            OnceAsync {
                count: count.clone(),
            },
        );
        let h = tokio::runtime::Handle::current();
        s.run_async(&mut w, &h).await.unwrap();
        s.run_async(&mut w, &h).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_async_enable_disable() {
        struct ToggleAsync {
            val: Arc<AtomicUsize>,
            enabled: bool,
        }
        impl AsyncSystem for ToggleAsync {
            fn name(&self) -> &'static str {
                "toggle_async"
            }
            fn enabled(&self) -> bool {
                self.enabled
            }
            fn set_enabled(&mut self, e: bool) {
                self.enabled = e;
            }
            fn run_async(
                &mut self,
                _world: &World,
            ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
                let v = self.val.clone();
                Box::pin(async move {
                    v.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }
        }

        let val = Arc::new(AtomicUsize::new(0));
        let mut w = World::new();
        let mut s = Scheduler::new();
        s.add_async(
            Stage::Update,
            ToggleAsync {
                val: val.clone(),
                enabled: true,
            },
        );
        let h = tokio::runtime::Handle::current();
        s.run_async(&mut w, &h).await.unwrap();
        assert_eq!(val.load(Ordering::SeqCst), 1);

        s.set_async_enabled("toggle_async", false);
        s.run_async(&mut w, &h).await.unwrap();
        assert_eq!(val.load(Ordering::SeqCst), 1);

        s.set_async_enabled("toggle_async", true);
        s.run_async(&mut w, &h).await.unwrap();
        assert_eq!(val.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_async_with_events() {
        let mut w = World::new();
        w.insert_raw(Arc::new(Events::<Ev>::new()));

        struct WriterAsync;
        impl AsyncSystem for WriterAsync {
            fn name(&self) -> &'static str {
                "writer_async"
            }
            fn run_async(
                &mut self,
                world: &World,
            ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
                // We need to get the Events from world inside the async block
                let (lock, _) = world.get_raw::<Events<Ev>>().unwrap();
                Box::pin(async move {
                    lock.send(Ev(100));
                    Ok(())
                })
            }
        }

        let mut s = Scheduler::new();
        s.add_async(Stage::Update, WriterAsync);
        let h = tokio::runtime::Handle::current();
        s.run_async(&mut w, &h).await.unwrap();

        let (lock, _) = w.get_raw::<Events<Ev>>().unwrap();
        let drained = lock.drain();
        assert_eq!(drained, vec![Ev(100)]);
    }
}
