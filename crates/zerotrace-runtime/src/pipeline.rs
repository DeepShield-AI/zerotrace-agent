// PipelineExecutor: connects Sources → Processors → Reporters via channels.
//
// Each stage runs as a tokio task. Channels are bounded with configurable
// capacity; backpressure propagates upstream when downstream consumers are
// slow.  Lifecycle hooks (start/stop) are called via the LifecycleRegistry
// pattern.
//
// # Architecture
//
// ```
// Source(s) ──mpsc──→ Processor(s) ──mpsc──→ Reporter(s)
// ```
//
// Multiple sources feed into a single processor channel. Each processor
// receives from that channel and pushes to the reporter channel. Multiple
// reporters drain the reporter channel independently.

use parking_lot::Mutex as ParkingMutex;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, mpsc, watch},
    task::JoinSet,
};
use zerotrace_core::{error::Result, signal::Batch};

// ── Backpressure policy ─────────────────────────────────────────────

/// Controls what happens when a pipeline channel reaches capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackpressurePolicy {
    /// Block the sender until space is available (default).
    /// Safe — no data loss.  Worst-case: backpressure propagates upstream.
    #[default]
    Block,
    /// Drop the oldest batch in the channel to make room.
    /// Useful when recent data is more valuable than old data.
    DropOldest,
    /// Drop the newest (incoming) batch.
    /// Useful when you'd rather keep a complete history and skip spikes.
    DropNewest,
}

// ── Stage traits (kept from original design, refined) ──────────────

/// A Source produces signals and pushes them downstream.
/// Implements the kernel's Lifecycle for managed start/stop.
pub trait Source: Send + 'static {
    fn name(&self) -> &'static str;
    fn run(
        &mut self,
        sink: mpsc::Sender<Batch>,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
    /// Called when shutdown is requested. Should cause `run()` to return.
    fn shutdown(&mut self) {}
}

/// A Processor transforms batches in-place.
pub trait Processor: Send + 'static {
    fn name(&self) -> &'static str;
    fn process(
        &mut self,
        batch: &mut Batch,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

/// A Reporter submits processed signals to an external sink.
pub trait Reporter: Send + 'static {
    fn name(&self) -> &'static str;
    fn submit(&mut self, batch: &Batch) -> impl std::future::Future<Output = Result<()>> + Send;
}

// ── Pipeline spec ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PipelineSpec {
    pub name: String,
    pub source_ids: Vec<String>,
    pub processor_ids: Vec<String>,
    pub reporter_ids: Vec<String>,
    pub channel_capacity: usize,
    pub enabled: bool,
    /// What to do when a channel reaches capacity.
    /// Default: [`BackpressurePolicy::Block`].
    pub backpressure: BackpressurePolicy,
}

impl Default for PipelineSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            source_ids: vec![],
            processor_ids: vec![],
            reporter_ids: vec![],
            channel_capacity: 4096,
            enabled: true,
            backpressure: BackpressurePolicy::Block,
        }
    }
}

// ── Backpressure channel ───────────────────────────────────────────

/// Internal shared queue for backpressure-aware channels.
struct BpInner<T> {
    queue: ParkingMutex<VecDeque<T>>,
    capacity: usize,
    policy: BackpressurePolicy,
    /// Wakes blocked receivers when data arrives.
    notify_rx: Notify,
    /// Wakes blocked senders (Block mode) when space frees.
    notify_tx: Notify,
    /// Number of live senders.
    senders_alive: AtomicUsize,
    /// Channel is closed (all senders dropped or explicit close).
    closed: AtomicBool,
}

/// Sender end of a backpressure-aware bounded channel.
///
/// Implements the configured [`BackpressurePolicy`] on every send:
/// - `Block`: waits for space (default mpsc behavior)
/// - `DropOldest`: evicts the oldest item when full
/// - `DropNewest`: discards the incoming item when full
pub struct BackpressureSender<T> {
    inner: Arc<BpInner<T>>,
}

impl<T> Clone for BackpressureSender<T> {
    fn clone(&self) -> Self {
        self.inner.senders_alive.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Drop for BackpressureSender<T> {
    fn drop(&mut self) {
        // AcqRel ensures we see all prior clone/drop operations and
        // that our decrement is visible to any concurrent drop that
        // might also observe 1 (though such races are inherent to
        // ref-counting — a new sender cloned after this check will
        // get a closed channel on first send).
        if self.inner.senders_alive.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.closed.store(true, Ordering::Release);
            // Wake ALL blocked receivers so they can observe `closed`
            // and return None.  notify_one() would leave additional
            // receivers blocked forever in a fan-out reporter topology.
            self.inner.notify_rx.notify_waiters();
        }
    }
}

impl<T: Clone + Send + 'static> BackpressureSender<T> {
    /// Send an item with the configured backpressure policy.
    pub async fn send(&self, item: T) -> std::result::Result<(), T> {
        match self.inner.policy {
            BackpressurePolicy::Block => self.send_block(item).await,
            BackpressurePolicy::DropOldest => {
                self.send_drop_oldest(item);
                Ok(())
            },
            BackpressurePolicy::DropNewest => {
                self.send_drop_newest(item);
                Ok(())
            },
        }
    }

    async fn send_block(&self, item: T) -> std::result::Result<(), T> {
        loop {
            {
                let mut q = self.inner.queue.lock();
                if q.len() < self.inner.capacity {
                    q.push_back(item);
                    self.inner.notify_rx.notify_one();
                    return Ok(());
                }
            }
            // Wait for space
            self.inner.notify_tx.notified().await;
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(item);
            }
        }
    }

    fn send_drop_oldest(&self, item: T) {
        let mut q = self.inner.queue.lock();
        if q.len() >= self.inner.capacity {
            q.pop_front(); // Evict oldest
        }
        q.push_back(item);
        self.inner.notify_rx.notify_one();
    }

    fn send_drop_newest(&self, item: T) {
        let mut q = self.inner.queue.lock();
        if q.len() >= self.inner.capacity {
            return; // Drop incoming
        }
        q.push_back(item);
        self.inner.notify_rx.notify_one();
    }

    /// Returns the number of items currently in the channel.
    pub fn len(&self) -> usize {
        self.inner.queue.lock().len()
    }

    /// Returns `true` if the channel contains no items.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Receiver end of a backpressure-aware bounded channel.
pub struct BackpressureReceiver<T> {
    inner: Arc<BpInner<T>>,
}

impl<T: Clone + Send + 'static> BackpressureReceiver<T> {
    /// Receive an item from the channel.
    /// Returns `None` when the channel is closed and empty.
    pub async fn recv(&mut self) -> Option<T> {
        loop {
            {
                let mut q = self.inner.queue.lock();
                if let Some(item) = q.pop_front() {
                    // Notify one blocked sender that space is available
                    self.inner.notify_tx.notify_one();
                    return Some(item);
                }
                if self.inner.closed.load(Ordering::Acquire) {
                    return None;
                }
            }
            self.inner.notify_rx.notified().await;
        }
    }

    /// Try to receive without blocking.
    pub fn try_recv(&mut self) -> Option<T> {
        let mut q = self.inner.queue.lock();
        let item = q.pop_front();
        if item.is_some() {
            self.inner.notify_tx.notify_one();
        }
        item
    }
}

/// Create a bounded backpressure channel with the given capacity and policy.
pub fn backpressure_channel<T: Clone + Send + 'static>(
    capacity: usize,
    policy: BackpressurePolicy,
) -> (BackpressureSender<T>, BackpressureReceiver<T>) {
    let inner = Arc::new(BpInner {
        queue: ParkingMutex::new(VecDeque::with_capacity(capacity)),
        capacity,
        policy,
        notify_rx: Notify::new(),
        notify_tx: Notify::new(),
        senders_alive: AtomicUsize::new(1),
        closed: AtomicBool::new(false),
    });
    let tx = BackpressureSender {
        inner: inner.clone(),
    };
    let rx = BackpressureReceiver { inner };
    (tx, rx)
}

// ── Type-erased pipeline stage wrappers ────────────────────────────

/// Type-erased future returned by [`BoxedSource::run`].
pub type SourceFuture = Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>;

/// Type-erased run-once callback for sources.
pub type SourceRunFn = Box<dyn FnOnce(mpsc::Sender<Batch>) -> SourceFuture + Send>;

/// A type-erased [`Source`] that can be spawned alongside other sources
/// of different concrete types.
///
/// Created via [`From`]`<S: Source>`.
///
/// # Semantics
///
/// `run()` and `shutdown()` each consume their internal closure on first
/// call (at-most-once).  This matches how [`PipelineExecutor`] uses sources:
/// a source task calls `run()` once, and `shutdown()` is called at most
/// once (only if the task is cancelled before `run()` completes).
pub struct BoxedSource {
    pub name: &'static str,
    run_fn: Option<SourceRunFn>,
    shutdown_fn: Option<Box<dyn FnOnce() + Send>>,
}

impl BoxedSource {
    /// Execute the source, feeding batches into `sink`.
    ///
    /// Returns an error if called more than once — each source is single-use.
    pub fn run(
        &mut self,
        sink: mpsc::Sender<Batch>,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
        match self.run_fn.take() {
            Some(f) => f(sink),
            None => Box::pin(async {
                Err(zerotrace_core::error::Error::Pipeline {
                    message: "BoxedSource::run called more than once".into(),
                    fatal: true,
                })
            }),
        }
    }

    /// Signal the source to stop producing.
    ///
    /// Safe to call multiple times; only the first call has an effect.
    pub fn shutdown(&mut self) {
        if let Some(f) = self.shutdown_fn.take() {
            f();
        }
    }
}

impl<S: Source> From<S> for BoxedSource {
    fn from(source: S) -> Self {
        let name = source.name();
        // The source is shared between run_fn and shutdown_fn via Arc<Mutex<Option>>.
        // run_fn takes the source out; shutdown_fn calls shutdown in-place.
        let inner: Arc<ParkingMutex<Option<S>>> = Arc::new(ParkingMutex::new(Some(source)));
        let run_inner = inner.clone();
        let shutdown_inner = inner.clone();
        BoxedSource {
            name,
            run_fn: Some(Box::new(move |sink| {
                let inner = run_inner.clone();
                Box::pin(async move {
                    let mut src = inner.lock().take().expect("source already taken");
                    src.run(sink).await
                })
            })),
            shutdown_fn: Some(Box::new(move || {
                if let Some(ref mut s) = *shutdown_inner.lock() {
                    s.shutdown();
                }
            })),
        }
    }
}

impl std::fmt::Debug for BoxedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxedSource").field("name", &self.name).finish()
    }
}

/// Type-erased future returned by [`BoxedProcessor::process`].
pub type ProcessFuture<'a> = Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;

/// Type-erased process callback.
pub type ProcessFn = Box<dyn for<'a> FnMut(&'a mut Batch) -> ProcessFuture<'a> + Send>;

/// A type-erased [`Processor`].
///
/// Created via [`From`]`<P: Processor>`.
///
/// `process()` may be called many times (once per incoming batch).
pub struct BoxedProcessor {
    pub name: &'static str,
    process_fn: Option<ProcessFn>,
}

impl BoxedProcessor {
    /// Process a batch of signals in-place.
    ///
    /// Returns an error if the processor has already been consumed.
    pub fn process<'a>(
        &'a mut self,
        batch: &'a mut Batch,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        match self.process_fn.as_mut() {
            Some(f) => f(batch),
            None => Box::pin(async {
                Err(zerotrace_core::error::Error::Pipeline {
                    message: "BoxedProcessor used after shutdown".into(),
                    fatal: true,
                })
            }),
        }
    }
}

impl<P: Processor> From<P> for BoxedProcessor {
    fn from(processor: P) -> Self {
        let name = processor.name();
        let inner: Arc<tokio::sync::Mutex<P>> = Arc::new(tokio::sync::Mutex::new(processor));
        let process_inner = inner.clone();
        BoxedProcessor {
            name,
            process_fn: Some(Box::new(move |batch| {
                let inner = process_inner.clone();
                Box::pin(async move {
                    let mut proc = inner.lock().await;
                    proc.process(batch).await
                })
            })),
        }
    }
}

impl std::fmt::Debug for BoxedProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxedProcessor").field("name", &self.name).finish()
    }
}

/// Type-erased future returned by [`BoxedReporter::submit`].
pub type SubmitFuture<'a> = Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;

/// Type-erased submit callback.
pub type SubmitFn = Box<dyn for<'a> FnMut(&'a Batch) -> SubmitFuture<'a> + Send>;

/// A type-erased [`Reporter`].
///
/// Created via [`From`]`<R: Reporter>`.
///
/// `submit()` may be called many times (once per processed batch).
pub struct BoxedReporter {
    pub name: &'static str,
    submit_fn: Option<SubmitFn>,
}

impl BoxedReporter {
    /// Submit a batch of signals to the external sink.
    ///
    /// Returns an error if the reporter has already been consumed.
    pub fn submit<'a>(
        &'a mut self,
        batch: &'a Batch,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        match self.submit_fn.as_mut() {
            Some(f) => f(batch),
            None => Box::pin(async {
                Err(zerotrace_core::error::Error::Pipeline {
                    message: "BoxedReporter used after shutdown".into(),
                    fatal: true,
                })
            }),
        }
    }
}

impl<R: Reporter> From<R> for BoxedReporter {
    fn from(reporter: R) -> Self {
        let name = reporter.name();
        let inner: Arc<tokio::sync::Mutex<R>> = Arc::new(tokio::sync::Mutex::new(reporter));
        let submit_inner = inner.clone();
        BoxedReporter {
            name,
            submit_fn: Some(Box::new(move |batch| {
                let inner = submit_inner.clone();
                Box::pin(async move {
                    let mut rep = inner.lock().await;
                    rep.submit(batch).await
                })
            })),
        }
    }
}

impl std::fmt::Debug for BoxedReporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxedReporter").field("name", &self.name).finish()
    }
}

// ── PipelineExecutor ───────────────────────────────────────────────

/// Aggregated pipeline metrics.  All fields are updated from spawned tasks
/// via `Arc`; read them from any thread at any time.
#[derive(Debug, Clone)]
pub struct PipelineMetrics {
    /// Total items emitted by sources.
    pub batches_produced: Arc<AtomicU64>,
    /// Total items successfully submitted by reporters.
    pub batches_consumed: Arc<AtomicU64>,
    /// Total batches dropped due to processor error.
    pub batches_dropped: Arc<AtomicU64>,
    /// Cumulative source→processor latency in nanoseconds.
    pub total_latency_ns: Arc<AtomicU64>,
    /// Total reporter errors.
    pub reporter_errors: Arc<AtomicU64>,
}

impl Default for PipelineMetrics {
    fn default() -> Self {
        Self {
            batches_produced: Arc::new(AtomicU64::new(0)),
            batches_consumed: Arc::new(AtomicU64::new(0)),
            batches_dropped: Arc::new(AtomicU64::new(0)),
            total_latency_ns: Arc::new(AtomicU64::new(0)),
            reporter_errors: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// A running pipeline.  Dropping the handle cancels all tasks.
pub struct PipelineHandle {
    /// Sender used to signal shutdown to source tasks.
    shutdown_tx: watch::Sender<bool>,
    /// All spawned tasks (sources, processors, reporters).
    tasks: JoinSet<()>,
    /// Pipeline observability.
    pub metrics: PipelineMetrics,
}

impl std::fmt::Debug for PipelineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineHandle")
            .field("is_shutdown", &self.is_shutdown())
            .finish()
    }
}

impl PipelineHandle {
    /// Send shutdown signal. Tasks will see the channel close and wrap up.
    /// To wait for all tasks to exit, call [`shutdown_timeout`](Self::shutdown_timeout)
    /// instead; to abort immediately, drop the handle.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Send shutdown signal and wait for all tasks to finish, with a
    /// per-task deadline.  Returns `Ok(())` when all tasks exit before
    /// the deadline, or `Err` with the number of tasks still running.
    pub async fn shutdown_timeout(&mut self, timeout: Duration) -> std::result::Result<(), usize> {
        let _ = self.shutdown_tx.send(true);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(self.tasks.len());
            }
            match tokio::time::timeout_at(deadline, self.tasks.join_next()).await {
                Ok(Some(Ok(()))) => continue,
                Ok(Some(Err(join_err))) => {
                    tracing::warn!("pipeline task panicked during shutdown: {}", join_err);
                    continue;
                },
                Ok(None) => return Ok(()),
                Err(_elapsed) => return Err(self.tasks.len()),
            }
        }
    }

    /// Returns true if the shutdown signal has been sent.
    pub fn is_shutdown(&self) -> bool {
        *self.shutdown_tx.borrow()
    }
}

impl Drop for PipelineHandle {
    fn drop(&mut self) {
        self.tasks.abort_all();
    }
}

/// Builds and runs pipelines.
pub struct PipelineExecutor;

impl PipelineExecutor {
    /// Wire up sources → processors → reporters and spawn all tasks.
    ///
    /// Returns a [`PipelineHandle`] that can be used to gracefully shut
    /// down the pipeline.
    ///
    /// Accepts type-erased [`BoxedSource`], [`BoxedProcessor`], and
    /// [`BoxedReporter`] values — different concrete types can be
    /// mixed in the same pipeline.  Convert via `From`/`Into`:
    ///
    /// ```ignore
    /// use zerotrace_runtime::pipeline::BoxedSource;
    /// let sources: Vec<(String, BoxedSource)> = vec![
    ///     ("cpu".into(), CpuSource::new().into()),
    ///     ("mem".into(), MemSource::new().into()),
    /// ];
    /// ```
    ///
    /// Channel backpressure is determined by [`PipelineSpec::backpressure`]:
    /// - `Block`: sender waits for space (no data loss, safe default)
    /// - `DropOldest`: oldest item evicted when channel is full
    /// - `DropNewest`: incoming item discarded when channel is full
    pub fn spawn(
        spec: &PipelineSpec,
        sources: Vec<(String, BoxedSource)>,
        processors: Vec<(String, BoxedProcessor)>,
        reporters: Vec<(String, BoxedReporter)>,
    ) -> PipelineHandle {
        let cap = spec.channel_capacity.max(16);
        let bp = spec.backpressure;

        // Use backpressure-aware channels
        let (source_tx, mut source_rx) = backpressure_channel::<Batch>(cap, bp);
        let (proc_tx, mut proc_rx) = backpressure_channel::<Batch>(cap, bp);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Pipeline metrics (shared across spawned tasks)
        let produced = Arc::new(AtomicU64::new(0));
        let consumed = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let total_latency = Arc::new(AtomicU64::new(0));
        let reporter_errs = Arc::new(AtomicU64::new(0));

        let mut tasks = JoinSet::new();

        // ── Filter sources/processors/reporters by spec ────────

        let selected_sources: Vec<_> = if spec.source_ids.is_empty() {
            sources.into_iter().map(|(_, s)| s).collect()
        } else {
            let ids: std::collections::HashSet<_> = spec.source_ids.iter().cloned().collect();
            sources.into_iter().filter(|(id, _)| ids.contains(id)).map(|(_, s)| s).collect()
        };

        let selected_processors: Vec<_> = if spec.processor_ids.is_empty() {
            processors.into_iter().map(|(_, p)| p).collect()
        } else {
            let ids: std::collections::HashSet<_> = spec.processor_ids.iter().cloned().collect();
            processors
                .into_iter()
                .filter(|(id, _)| ids.contains(id))
                .map(|(_, p)| p)
                .collect()
        };

        let selected_reporters: Vec<_> = if spec.reporter_ids.is_empty() {
            reporters.into_iter().map(|(_, r)| r).collect()
        } else {
            let ids: std::collections::HashSet<_> = spec.reporter_ids.iter().cloned().collect();
            reporters
                .into_iter()
                .filter(|(id, _)| ids.contains(id))
                .map(|(_, r)| r)
                .collect()
        };

        // ── Spawn sources ─────────────────────────────────────
        // Sources write to a small mpsc bridge (capacity 1) for trait
        // compatibility. The bridge immediately forwards to the
        // backpressure channel so the policy applies with minimal latency.

        // Bridge uses the same capacity as the main channels so sources
        // don't stall on a bottleneck narrower than the pipeline itself.
        let (mpsc_tx, mut mpsc_rx) = mpsc::channel::<Batch>(cap);

        // Bridge task: drain mpsc → send with backpressure to source_tx
        let bridge_tx = source_tx.clone();
        let mut bridge_shutdown = shutdown_rx.clone();
        let produced_m = produced.clone();
        tasks.spawn(async move {
            loop {
                let batch_opt = tokio::select! {
                    biased;
                    _ = bridge_shutdown.changed() => None,
                    batch = mpsc_rx.recv() => batch,
                };
                match batch_opt {
                    Some(batch) => {
                        let count = batch.len() as u64;
                        if bridge_tx.send(batch).await.is_err() {
                            break;
                        }
                        produced_m.fetch_add(count, Ordering::Relaxed);
                    },
                    None => break,
                }
            }
        });

        for mut source in selected_sources {
            let tx = mpsc_tx.clone();
            let mut shutdown = shutdown_rx.clone();
            tasks.spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.changed() => {
                            if *shutdown.borrow() { source.shutdown(); break; }
                        }
                        result = source.run(tx.clone()) => {
                            if let Err(e) = result {
                                tracing::warn!("source [{}] error: {}", source.name, e);
                            }
                            break;
                        }
                    }
                }
            });
        }

        drop(mpsc_tx);

        // ── Spawn processors ──────────────────────────────────

        let mut processors = selected_processors;
        let mut shutdown_p = shutdown_rx.clone();
        let pt = proc_tx.clone();
        let dropped_m = dropped.clone();
        let latency_m = total_latency.clone();

        tasks.spawn(async move {
            loop {
                let batch_opt = tokio::select! {
                    biased;
                    _ = shutdown_p.changed() => None,
                    batch = source_rx.recv() => batch,
                };
                match batch_opt {
                    Some(mut batch) => {
                        let t0 = std::time::Instant::now();
                        let mut batch_valid = true;
                        for proc in processors.iter_mut() {
                            if let Err(e) = proc.process(&mut batch).await {
                                tracing::warn!(
                                    "processor [{}] error: {} — batch dropped",
                                    proc.name,
                                    e
                                );
                                batch_valid = false;
                                break;
                            }
                        }
                        latency_m.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
                        if batch_valid {
                            if pt.send(batch).await.is_err() {
                                break;
                            }
                        } else {
                            dropped_m.fetch_add(1, Ordering::Relaxed);
                        }
                    },
                    None => break,
                }
            }
        });

        drop(proc_tx);

        // ── Spawn reporters ───────────────────────────────────

        let mut reporters = selected_reporters;
        let mut shutdown_r = shutdown_rx.clone();
        let consumed_m = consumed.clone();
        let reporter_errs_m = reporter_errs.clone();

        tasks.spawn(async move {
            loop {
                let batch_opt = tokio::select! {
                    biased;
                    _ = shutdown_r.changed() => None,
                    batch = proc_rx.recv() => batch,
                };
                match batch_opt {
                    Some(batch) => {
                        let count = batch.len() as u64;
                        for rep in reporters.iter_mut() {
                            if let Err(e) = rep.submit(&batch).await {
                                tracing::warn!("reporter [{}] error: {}", rep.name, e);
                                reporter_errs_m.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        consumed_m.fetch_add(count, Ordering::Relaxed);
                    },
                    None => break,
                }
            }
        });

        PipelineHandle {
            shutdown_tx,
            tasks,
            metrics: PipelineMetrics {
                batches_produced: produced,
                batches_consumed: consumed,
                batches_dropped: dropped,
                total_latency_ns: total_latency,
                reporter_errors: reporter_errs,
            },
        }
    }
}

// ── Simple Source/Processor/Reporter implementations for testing ───

/// A source that emits items from an iterator.
pub struct IterSource<I: IntoIterator<Item = Batch> + Send + 'static> {
    pub name: &'static str,
    pub iter: Option<I>,
    pub delay: Option<Duration>,
}

impl<I: IntoIterator<Item = Batch> + Send + 'static> IterSource<I> {
    pub fn new(name: &'static str, iter: I) -> Self {
        Self {
            name,
            iter: Some(iter),
            delay: None,
        }
    }
    pub fn with_delay(mut self, d: Duration) -> Self {
        self.delay = Some(d);
        self
    }
}

impl<I: IntoIterator<Item = Batch> + Send + 'static> Source for IterSource<I>
where
    <I as IntoIterator>::IntoIter: Send,
{
    fn name(&self) -> &'static str {
        self.name
    }
    async fn run(&mut self, sink: mpsc::Sender<Batch>) -> Result<()> {
        if let Some(iter) = self.iter.take() {
            for batch in iter {
                if let Some(d) = self.delay {
                    tokio::time::sleep(d).await;
                }
                if sink.send(batch).await.is_err() {
                    break;
                }
            }
        }
        Ok(())
    }
}

/// A processor that applies a closure to each batch.
pub struct FnProcessor<F: FnMut(&mut Batch) -> Result<()> + Send + 'static> {
    pub name: &'static str,
    pub f: F,
}

impl<F: FnMut(&mut Batch) -> Result<()> + Send + 'static> FnProcessor<F> {
    pub fn new(name: &'static str, f: F) -> Self {
        Self { name, f }
    }
}

impl<F: FnMut(&mut Batch) -> Result<()> + Send + 'static> Processor for FnProcessor<F> {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn process(&mut self, batch: &mut Batch) -> Result<()> {
        (self.f)(batch)
    }
}

/// A reporter that collects batches into a Vec.
#[derive(Clone)]
pub struct CollectingReporter {
    pub name: &'static str,
    pub batches: Arc<parking_lot::Mutex<Vec<Batch>>>,
}

impl CollectingReporter {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            batches: Arc::new(parking_lot::Mutex::new(Vec::new())),
        }
    }
    pub fn collected(&self) -> Vec<Batch> {
        self.batches.lock().clone()
    }
    pub fn total_items(&self) -> usize {
        self.batches.lock().iter().map(|b| b.len()).sum()
    }
}

impl Reporter for CollectingReporter {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn submit(&mut self, batch: &Batch) -> Result<()> {
        self.batches.lock().push(batch.clone());
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use zerotrace_core::signal::{BatchMetadata, SignalKind};

    fn make_batch(_kind: SignalKind) -> Batch {
        Batch {
            items: vec![],
            metadata: Arc::new(BatchMetadata::new("test")),
        }
    }

    #[tokio::test]
    async fn test_pipeline_end_to_end() {
        let source = IterSource::new("test_src", vec![make_batch(SignalKind::METRIC)]);

        let processor =
            FnProcessor::new("test_proc", |_batch: &mut Batch| -> Result<()> { Ok(()) });

        let reporter = CollectingReporter::new("test_rep");
        let batches = reporter.batches.clone();

        let spec = PipelineSpec {
            name: "test".into(),
            source_ids: vec![],
            processor_ids: vec![],
            reporter_ids: vec![],
            channel_capacity: 16,
            enabled: true,
            backpressure: BackpressurePolicy::Block,
        };

        let handle = PipelineExecutor::spawn(
            &spec,
            vec![("s1".into(), source.into())],
            vec![("p1".into(), processor.into())],
            vec![("r1".into(), reporter.into())],
        );

        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(batches.lock().len(), 1);
    }

    #[tokio::test]
    async fn test_pipeline_multiple_batches() {
        let source = IterSource::new(
            "multi_src",
            vec![
                make_batch(SignalKind::LOG),
                make_batch(SignalKind::TRACE),
                make_batch(SignalKind::EVENT),
            ],
        );

        let processor = FnProcessor::new("noop", |_batch: &mut Batch| -> Result<()> { Ok(()) });
        let reporter = CollectingReporter::new("collector");
        let batches = reporter.batches.clone();

        let spec = PipelineSpec {
            name: "multi".into(),
            ..Default::default()
        };

        let handle = PipelineExecutor::spawn(
            &spec,
            vec![("s1".into(), source.into())],
            vec![("p1".into(), processor.into())],
            vec![("r1".into(), reporter.into())],
        );

        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(batches.lock().len(), 3);
    }

    #[tokio::test]
    async fn test_pipeline_spec_filter_by_ids() {
        let source1 = IterSource::new("s1", vec![make_batch(SignalKind::METRIC)]);
        let source2 = IterSource::new("s2", vec![make_batch(SignalKind::METRIC)]);

        let reporter = CollectingReporter::new("r1");
        let batches = reporter.batches.clone();

        let spec = PipelineSpec {
            name: "filtered".into(),
            source_ids: vec!["s1".into()],
            processor_ids: vec![],
            reporter_ids: vec!["r1".into()],
            channel_capacity: 16,
            enabled: true,
            backpressure: BackpressurePolicy::Block,
        };

        let handle = PipelineExecutor::spawn(
            &spec,
            vec![("s1".into(), source1.into()), ("s2".into(), source2.into())],
            vec![],
            vec![("r1".into(), reporter.into())],
        );

        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(batches.lock().len(), 1);
    }

    // ── Backpressure channel unit tests ──────────────────────────────

    #[tokio::test]
    async fn test_backpressure_channel_block_mode() {
        let (tx, mut rx) = backpressure_channel::<u32>(2, BackpressurePolicy::Block);

        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        assert_eq!(tx.len(), 2);

        assert_eq!(rx.recv().await, Some(1));
        assert_eq!(rx.recv().await, Some(2));
    }

    #[tokio::test]
    async fn test_backpressure_channel_drop_newest() {
        let (tx, mut rx) = backpressure_channel::<u32>(2, BackpressurePolicy::DropNewest);

        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        tx.send(3).await.unwrap(); // Should be dropped, channel full

        assert_eq!(tx.len(), 2);
        assert_eq!(rx.recv().await, Some(1));
        assert_eq!(rx.recv().await, Some(2));
        // 3 was dropped
        drop(tx);
        assert_eq!(rx.recv().await, None);
    }

    #[tokio::test]
    async fn test_backpressure_channel_drop_oldest() {
        let (tx, mut rx) = backpressure_channel::<u32>(2, BackpressurePolicy::DropOldest);

        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        tx.send(3).await.unwrap(); // Evicts 1 (oldest), queue becomes [2, 3]

        assert_eq!(tx.len(), 2);
        assert_eq!(rx.recv().await, Some(2)); // 1 was dropped
        assert_eq!(rx.recv().await, Some(3));
    }

    #[tokio::test]
    async fn test_backpressure_channel_try_recv() {
        let (tx, mut rx) = backpressure_channel::<u32>(4, BackpressurePolicy::Block);

        tx.send(10).await.unwrap();
        tx.send(20).await.unwrap();

        assert_eq!(rx.try_recv(), Some(10));
        assert_eq!(rx.try_recv(), Some(20));
        assert_eq!(rx.try_recv(), None);
    }

    #[tokio::test]
    async fn test_backpressure_channel_closed_on_drop() {
        let (tx, mut rx) = backpressure_channel::<u32>(4, BackpressurePolicy::Block);

        tx.send(1).await.unwrap();
        drop(tx); // Close channel

        assert_eq!(rx.recv().await, Some(1));
        assert_eq!(rx.recv().await, None); // Closed
    }

    #[tokio::test]
    async fn test_backpressure_channel_multi_sender() {
        let (tx1, mut rx) = backpressure_channel::<u32>(4, BackpressurePolicy::Block);
        let tx2 = tx1.clone();
        let tx3 = tx1.clone();

        tx1.send(1).await.unwrap();
        tx2.send(2).await.unwrap();
        tx3.send(3).await.unwrap();
        drop(tx1);
        drop(tx2);
        drop(tx3); // Last sender — closes

        let mut results = vec![];
        while let Some(v) = rx.recv().await {
            results.push(v);
        }
        assert_eq!(results.len(), 3);
    }

    // ── Pipeline backpressure integration tests ─────────────────────

    /// A processor that sleeps to create backpressure.
    struct SlowProcessor {
        pub delay_ms: u64,
    }

    impl Processor for SlowProcessor {
        fn name(&self) -> &'static str {
            "slow_proc"
        }
        async fn process(&mut self, _batch: &mut Batch) -> Result<()> {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_pipeline_drop_newest_backpressure() {
        // Create many batches, small channel + slow processor → backpressure
        let batches: Vec<Batch> = (0..30)
            .map(|i| Batch {
                metadata: Arc::new(BatchMetadata::new("test")),
                items: vec![],
            })
            .collect();

        let source = IterSource::new("flood_src", batches);

        // Processor takes 10ms per batch → with channel capacity 2,
        // the source (no delay) floods the channel quickly
        let processor = SlowProcessor { delay_ms: 5 };

        let reporter = CollectingReporter::new("collector");
        let batches_arc = reporter.batches.clone();

        let spec = PipelineSpec {
            name: "drop_newest".into(),
            channel_capacity: 2,
            backpressure: BackpressurePolicy::DropNewest,
            ..Default::default()
        };

        let handle = PipelineExecutor::spawn(
            &spec,
            vec![("s1".into(), source.into())],
            vec![("p1".into(), processor.into())],
            vec![("r1".into(), reporter.into())],
        );

        // Wait long enough for pipeline to process but not all batches
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let received = batches_arc.lock().len();
        assert!(
            received < 30,
            "DropNewest should drop some batches, got {received}/30"
        );
        assert!(received > 0, "Should receive at least some batches");
    }

    #[tokio::test]
    async fn test_pipeline_drop_oldest_backpressure() {
        let batches: Vec<Batch> = (0..30)
            .map(|i| Batch {
                metadata: Arc::new(BatchMetadata::new("test")),
                items: vec![],
            })
            .collect();

        let source = IterSource::new("flood_src", batches);

        let processor = SlowProcessor { delay_ms: 5 };

        let reporter = CollectingReporter::new("collector");
        let batches_arc = reporter.batches.clone();

        let spec = PipelineSpec {
            name: "drop_oldest".into(),
            channel_capacity: 2,
            backpressure: BackpressurePolicy::DropOldest,
            ..Default::default()
        };

        let handle = PipelineExecutor::spawn(
            &spec,
            vec![("s1".into(), source.into())],
            vec![("p1".into(), processor.into())],
            vec![("r1".into(), reporter.into())],
        );

        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.shutdown();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let received = batches_arc.lock().len();
        assert!(
            received < 30,
            "DropOldest should evict some batches, got {received}/30"
        );
        assert!(received > 0, "Should receive at least some batches");
    }

    // ── Heterogeneous pipeline test ────────────────────────────────

    /// A second source type (different from IterSource) for testing
    /// heterogeneous pipelines.
    struct ConstSource {
        pub batch: Batch,
        pub repeat: usize,
    }

    impl Source for ConstSource {
        fn name(&self) -> &'static str {
            "const_src"
        }
        async fn run(&mut self, sink: mpsc::Sender<Batch>) -> Result<()> {
            for _ in 0..self.repeat {
                if sink.send(self.batch.clone()).await.is_err() {
                    break;
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_pipeline_heterogeneous_sources() {
        // Mix IterSource and ConstSource in the same pipeline
        let source1 = IterSource::new("iter_src", vec![make_batch(SignalKind::METRIC)]);
        let source2 = ConstSource {
            batch: make_batch(SignalKind::LOG),
            repeat: 2,
        };

        let reporter = CollectingReporter::new("collector");
        let batches = reporter.batches.clone();

        let spec = PipelineSpec {
            name: "hetero".into(),
            ..Default::default()
        };

        let handle = PipelineExecutor::spawn(
            &spec,
            vec![("s1".into(), source1.into()), ("s2".into(), source2.into())],
            vec![],
            vec![("r1".into(), reporter.into())],
        );

        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 1 batch from IterSource + 2 from ConstSource = 3 total
        assert_eq!(batches.lock().len(), 3);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Typed pipeline (Phase 3) — async_trait for dyn compatibility
// ═══════════════════════════════════════════════════════════════════════

use zerotrace_core::signal::{SignalType, TypedBatch};

/// A typed pipeline handle.
pub struct TypedPipelineHandle {
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    tasks: tokio::task::JoinSet<()>,
}

impl TypedPipelineHandle {
    /// Send the shutdown signal to all pipeline tasks.  Does not wait
    /// for tasks to exit — drop the handle (which calls [`JoinSet::abort_all`])
    /// to force-stop.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl Drop for TypedPipelineHandle {
    fn drop(&mut self) {
        self.tasks.abort_all();
    }
}

/// Trait for typed sources that produce [`TypedBatch<T>`].
#[async_trait::async_trait]
pub trait TypedSource<T: SignalType>: Send + 'static {
    fn name(&self) -> &'static str;
    async fn run(
        &mut self,
        sink: tokio::sync::mpsc::Sender<TypedBatch<T>>,
    ) -> zerotrace_core::error::Result<()>;
    fn shutdown(&mut self) {}
}

/// Trait for typed processors that transform [`TypedBatch<T>`] in-place.
#[async_trait::async_trait]
pub trait TypedProcessor<T: SignalType>: Send + 'static {
    fn name(&self) -> &'static str;
    async fn process(&mut self, batch: &mut TypedBatch<T>) -> zerotrace_core::error::Result<()>;
}

/// Trait for typed reporters that submit [`TypedBatch<T>`].
#[async_trait::async_trait]
pub trait TypedReporter<T: SignalType>: Send + 'static {
    fn name(&self) -> &'static str;
    async fn submit(&mut self, batch: &TypedBatch<T>) -> zerotrace_core::error::Result<()>;
}

/// Builds and runs typed pipelines.
pub struct TypedPipelineExecutor;

impl TypedPipelineExecutor {
    /// Spawn a typed pipeline: source → processors → reporters.
    ///
    /// All stages are monomorphised on `T: SignalType`, so there is no
    /// per-item enum dispatch on the hot path.
    pub fn spawn<T: SignalType>(
        mut source: Box<dyn TypedSource<T>>,
        mut processors: Vec<Box<dyn TypedProcessor<T>>>,
        mut reporters: Vec<Box<dyn TypedReporter<T>>>,
        channel_capacity: usize,
    ) -> TypedPipelineHandle {
        let cap = channel_capacity.max(16);
        let (source_tx, mut source_rx) = tokio::sync::mpsc::channel::<TypedBatch<T>>(cap);
        let (proc_tx, mut proc_rx) = tokio::sync::mpsc::channel::<TypedBatch<T>>(cap);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let mut tasks = tokio::task::JoinSet::new();

        // ── Source task ────────────────────────────────────────────
        let mut src_shutdown = shutdown_rx.clone();
        tasks.spawn(async move {
            tokio::select! {
                _ = src_shutdown.changed() => {
                    if *src_shutdown.borrow() {
                        source.shutdown();
                    }
                }
                result = source.run(source_tx) => {
                    if let Err(e) = result {
                        tracing::warn!("typed source [{}] error: {}", source.name(), e);
                    }
                }
            }
        });

        // ── Processor task ─────────────────────────────────────────
        let mut proc_shutdown = shutdown_rx.clone();
        tasks.spawn(async move {
            loop {
                let batch_opt = tokio::select! {
                    _ = proc_shutdown.changed() => None,
                    batch = source_rx.recv() => batch,
                };
                match batch_opt {
                    Some(mut batch) => {
                        for proc in processors.iter_mut() {
                            if let Err(e) = proc.process(&mut batch).await {
                                tracing::warn!("typed processor [{}] error: {}", proc.name(), e);
                            }
                        }
                        if proc_tx.send(batch).await.is_err() {
                            break;
                        }
                    },
                    None => break,
                }
            }
        });
        // (proc_tx moved into processor task — dropped when task ends)

        // ── Reporter task ──────────────────────────────────────────
        let mut rep_shutdown = shutdown_rx;
        tasks.spawn(async move {
            loop {
                let batch_opt = tokio::select! {
                    _ = rep_shutdown.changed() => None,
                    batch = proc_rx.recv() => batch,
                };
                match batch_opt {
                    Some(batch) =>
                        for rep in reporters.iter_mut() {
                            if let Err(e) = rep.submit(&batch).await {
                                tracing::warn!("typed reporter [{}] error: {}", rep.name(), e);
                            }
                        },
                    None => break,
                }
            }
        });

        TypedPipelineHandle { shutdown_tx, tasks }
    }
}

// ── Typed pipeline tests ────────────────────────────────────────────

#[cfg(test)]
mod typed_tests {
    use super::*;
    use std::borrow::Cow;
    use zerotrace_core::signal::{AttrValue, BatchMetadata, MetricPoint};

    struct FixedMetricSource {
        batches: Vec<TypedBatch<MetricPoint>>,
    }

    #[async_trait::async_trait]
    impl TypedSource<MetricPoint> for FixedMetricSource {
        fn name(&self) -> &'static str {
            "fixed_metrics"
        }
        async fn run(
            &mut self,
            sink: tokio::sync::mpsc::Sender<TypedBatch<MetricPoint>>,
        ) -> zerotrace_core::error::Result<()> {
            for batch in self.batches.drain(..) {
                if sink.send(batch).await.is_err() {
                    break;
                }
            }
            Ok(())
        }
    }

    struct TagProcessor {
        key: &'static str,
        val: &'static str,
    }
    #[async_trait::async_trait]
    impl TypedProcessor<MetricPoint> for TagProcessor {
        fn name(&self) -> &'static str {
            "tag"
        }
        async fn process(
            &mut self,
            batch: &mut TypedBatch<MetricPoint>,
        ) -> zerotrace_core::error::Result<()> {
            for item in batch.items.iter_mut() {
                item.attributes.push((
                    Cow::Borrowed(self.key),
                    AttrValue::Str(Cow::Borrowed(self.val)),
                ));
            }
            Ok(())
        }
    }

    struct CollectingMetricReporter {
        received: Arc<parking_lot::Mutex<Vec<TypedBatch<MetricPoint>>>>,
    }
    #[async_trait::async_trait]
    impl TypedReporter<MetricPoint> for CollectingMetricReporter {
        fn name(&self) -> &'static str {
            "collect"
        }
        async fn submit(
            &mut self,
            batch: &TypedBatch<MetricPoint>,
        ) -> zerotrace_core::error::Result<()> {
            self.received.lock().push(batch.clone());
            Ok(())
        }
    }

    fn make_source(n: usize) -> FixedMetricSource {
        let meta = Arc::new(BatchMetadata::new("test"));
        let mut batches = Vec::new();
        for i in 0..n {
            let mut batch = TypedBatch::<MetricPoint>::new(meta.clone());
            batch.push(MetricPoint::gauge("m", i as f64, i as i64 * 1000));
            batches.push(batch);
        }
        FixedMetricSource { batches }
    }

    #[tokio::test]
    async fn typed_pipeline_end_to_end() {
        let received = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let reporter = CollectingMetricReporter {
            received: received.clone(),
        };

        let handle = TypedPipelineExecutor::spawn::<MetricPoint>(
            Box::new(make_source(3)),
            vec![Box::new(TagProcessor {
                key: "env",
                val: "test",
            })],
            vec![Box::new(reporter)],
            32,
        );

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        handle.shutdown();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let batches = received.lock();
        assert_eq!(batches.len(), 3);
        for batch in batches.iter() {
            assert_eq!(batch.len(), 1);
            assert!(
                batch.items[0]
                    .attributes
                    .iter()
                    .any(|(k, v)| k == "env" && v.as_str() == Some("test"))
            );
        }
    }

    #[tokio::test]
    async fn typed_pipeline_empty_graceful() {
        let received = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let handle = TypedPipelineExecutor::spawn::<MetricPoint>(
            Box::new(make_source(0)),
            vec![],
            vec![Box::new(CollectingMetricReporter {
                received: received.clone(),
            })],
            16,
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        handle.shutdown();
        assert!(received.lock().is_empty());
    }

    #[tokio::test]
    async fn typed_pipeline_multiple_processors() {
        let received = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let handle = TypedPipelineExecutor::spawn::<MetricPoint>(
            Box::new(make_source(1)),
            vec![
                Box::new(TagProcessor {
                    key: "p1",
                    val: "a",
                }),
                Box::new(TagProcessor {
                    key: "p2",
                    val: "b",
                }),
            ],
            vec![Box::new(CollectingMetricReporter {
                received: received.clone(),
            })],
            32,
        );

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        handle.shutdown();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tags = &received.lock()[0].items[0].attributes;
        assert!(tags.iter().any(|(k, v)| k == "p1"));
        assert!(tags.iter().any(|(k, v)| k == "p2"));
    }
}
