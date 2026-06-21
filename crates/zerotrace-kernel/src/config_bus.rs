// ConfigBus: subscription-based configuration change distribution.
// ConfigRepo: YAML/JSON file-backed config with change detection.

use crate::{
    error::{Error, Result},
    lifecycle::LifecycleCtx,
    world::World,
};
use serde::de::DeserializeOwned;
use std::{fs, path::PathBuf, time::SystemTime};
use tokio::time::{Duration, interval};

#[derive(Debug, Clone)]
pub enum ConfigChange {
    Replaced {
        type_name: &'static str,
    },
    Field {
        path: Vec<String>,
        old_value: serde_json::Value,
        new_value: serde_json::Value,
    },
    FullReload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    HotApplied,
    RestartSelf,
    RestartPipeline(&'static str),
    RestartAgent,
}

impl Action {
    pub fn severity(&self) -> u8 {
        match self {
            Action::HotApplied => 0,
            Action::RestartSelf => 1,
            Action::RestartPipeline(_) => 2,
            Action::RestartAgent => 3,
        }
    }
    pub fn is_more_severe_than(&self, other: &Action) -> bool {
        self.severity() > other.severity()
    }
}

#[async_trait::async_trait]
pub trait ConfigSubscriber: Send + Sync {
    fn name(&self) -> &'static str;
    fn interested(&self, change: &ConfigChange) -> bool;
    async fn on_change(&mut self, change: &ConfigChange, ctx: &LifecycleCtx) -> Result<Action>;
}

pub struct ConfigBus {
    subscribers: Vec<Box<dyn ConfigSubscriber>>,
}

impl ConfigBus {
    pub fn new() -> Self {
        Self {
            subscribers: Vec::new(),
        }
    }
    pub fn subscribe<S: ConfigSubscriber + 'static>(&mut self, sub: S) {
        self.subscribers.push(Box::new(sub));
    }
    pub fn len(&self) -> usize {
        self.subscribers.len()
    }
    pub fn is_empty(&self) -> bool {
        self.subscribers.is_empty()
    }

    /// Dispatch a config change to all subscribers.
    ///
    /// Returns the most severe [`Action`] across all interested
    /// subscribers.  Short-circuits on [`Action::RestartAgent`] (the
    /// maximum severity), skipping remaining subscribers — no action can
    /// be more severe, so further notification adds no value and may
    /// waste work that will be torn down by the restart anyway.
    pub async fn dispatch(&mut self, change: &ConfigChange, ctx: &LifecycleCtx) -> Result<Action> {
        let start = std::time::Instant::now();
        let mut max = Action::HotApplied;
        for sub in self.subscribers.iter_mut() {
            if !sub.interested(change) {
                continue;
            }
            let act = sub.on_change(change, ctx).await?;
            if act.is_more_severe_than(&max) {
                max = act;
            }
            if matches!(max, Action::RestartAgent) {
                // Nothing can be more severe — stop notifying.
                break;
            }
        }
        let elapsed_ms = start.elapsed().as_millis() as u64;
        let m = ctx.world.metrics();
        m.config_dispatch_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        m.config_dispatch_total_ms
            .fetch_add(elapsed_ms, std::sync::atomic::Ordering::Relaxed);
        Ok(max)
    }
}

impl Default for ConfigBus {
    fn default() -> Self {
        Self::new()
    }
}

// ── ConfigSource ──────────────────────────────────────────────────────

/// Abstraction over configuration sources.
///
/// Implement this trait to support additional config sources (etcd,
/// consul, environment variables, CLI flags, etc.).
pub trait ConfigSource: Send + Sync {
    /// Read the raw configuration content as a string.
    fn read_raw(&self) -> Result<String>;
    /// Return the last-modified timestamp, if available.
    /// Used for change detection; returning `None` disables mtime-based
    /// optimisation (the repo will fall back to content hashing).
    fn last_modified(&self) -> Option<SystemTime> {
        None
    }
    /// Human-readable label for logging / error messages.
    fn label(&self) -> &str {
        "config_source"
    }
}

/// File-based config source.
pub struct FileSource {
    path: PathBuf,
}

impl FileSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl ConfigSource for FileSource {
    fn read_raw(&self) -> Result<String> {
        fs::read_to_string(&self.path).map_err(Error::Io)
    }
    fn last_modified(&self) -> Option<SystemTime> {
        fs::metadata(&self.path).ok().and_then(|m| m.modified().ok())
    }
    fn label(&self) -> &str {
        self.path.to_str().unwrap_or("file_source")
    }
}

/// Environment-variable config source.
///
/// Reads all env vars with the given prefix and builds a JSON string
/// from their values (stripping the prefix from keys).  Change detection
/// is handled by [`ConfigRepo::check`] via content hashing, so no
/// caching is needed inside the source itself.
pub struct EnvSource {
    prefix: String,
}

impl EnvSource {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

impl ConfigSource for EnvSource {
    fn read_raw(&self) -> Result<String> {
        let prefix = format!("{}_", self.prefix);
        let mut map = serde_json::Map::new();
        for (key, value) in std::env::vars() {
            if let Some(stripped) = key.strip_prefix(&prefix) {
                map.insert(stripped.to_lowercase(), serde_json::Value::String(value));
            }
        }
        serde_json::to_string(&serde_json::Value::Object(map))
            .map_err(|e| Error::Config(format!("env serialization: {}", e)))
    }
    fn last_modified(&self) -> Option<SystemTime> {
        // Env vars don't have mtime — we use content hash for
        // change detection.
        None
    }
    fn label(&self) -> &str {
        "env_source"
    }
}

/// Static config source (for hard-coded defaults or test fixtures).
pub struct StaticSource {
    content: String,
    label: &'static str,
}

impl StaticSource {
    pub fn new(content: impl Into<String>, label: &'static str) -> Self {
        Self {
            content: content.into(),
            label,
        }
    }
}

impl ConfigSource for StaticSource {
    fn read_raw(&self) -> Result<String> {
        Ok(self.content.clone())
    }
    fn label(&self) -> &str {
        self.label
    }
}

// ── ConfigRepo ───────────────────────────────────────────────────────

pub struct ConfigRepo<T: DeserializeOwned + Send + Sync + 'static> {
    source: Box<dyn ConfigSource>,
    last_mtime: Option<SystemTime>,
    last_hash: u64,
    poll_interval: Duration,
    _marker: std::marker::PhantomData<T>,
}

impl<T: DeserializeOwned + Send + Sync + 'static> ConfigRepo<T> {
    /// Create a ConfigRepo from a file path (backward-compatible).
    pub fn new(path: impl Into<PathBuf>, world: &World) -> Result<Self> {
        Self::from_source(Box::new(FileSource::new(path)), world)
    }

    /// Create a ConfigRepo from an arbitrary [`ConfigSource`].
    pub fn from_source(source: Box<dyn ConfigSource>, world: &World) -> Result<Self> {
        let content = source.read_raw()?;
        let value: T = serde_yaml::from_str(&content)
            .or_else(|_| serde_json::from_str(&content))
            .map_err(|e| Error::Config(format!("parse {}: {}", source.label(), e)))?;
        let mtime = source.last_modified();
        let hash = Self::hash(&content);
        world.insert(value);
        Ok(Self {
            source,
            last_mtime: mtime,
            last_hash: hash,
            poll_interval: Duration::from_secs(5),
            _marker: std::marker::PhantomData,
        })
    }

    /// Create a ConfigRepo from a file (convenience alias).
    pub fn from_file(path: impl Into<PathBuf>, world: &World) -> Result<Self> {
        Self::from_source(Box::new(FileSource::new(path)), world)
    }

    /// Create a ConfigRepo from environment variables with the given
    /// prefix.  Keys are lowercased and the prefix is stripped.
    pub fn from_env(prefix: impl Into<String>, world: &World) -> Result<Self> {
        Self::from_source(Box::new(EnvSource::new(prefix)), world)
    }

    pub fn poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    pub async fn watch(
        &mut self,
        world: &World,
        bus: &mut ConfigBus,
        ctx: &LifecycleCtx,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        let mut tick = interval(self.poll_interval);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Some(change) = self.check(world)? { bus.dispatch(&change, ctx).await?; }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() { return Ok(()); }
                }
            }
        }
    }

    /// Check for config changes.  Returns `Some(change)` when new
    /// content is detected, or `None` when the content is unchanged.
    /// Public for integration testing.
    pub fn check(&mut self, world: &World) -> Result<Option<ConfigChange>> {
        // Fast path: mtime-based skip for file sources.
        // Non-file sources (EnvSource, etc.) always return None here
        // and fall through to hash-based change detection below.
        let cur_mtime = self.source.last_modified();
        if cur_mtime.is_some() && cur_mtime == self.last_mtime {
            return Ok(None);
        }

        let content = self.source.read_raw()?;
        let cur_hash = Self::hash(&content);

        // Hash-based deduplication: avoids re-parsing identical content.
        // This is the primary change-detection mechanism for non-file
        // sources (env vars, etcd) where mtime is unavailable.
        if cur_hash == self.last_hash {
            self.last_mtime = cur_mtime;
            return Ok(None);
        }

        let value: T =
            serde_yaml::from_str(&content)
                .or_else(|_| serde_json::from_str(&content))
                .map_err(|e| Error::Config(format!("parse {}: {}", self.source.label(), e)))?;
        self.last_mtime = cur_mtime;
        self.last_hash = cur_hash;
        world.insert(value);
        Ok(Some(ConfigChange::Replaced {
            type_name: std::any::type_name::<T>(),
        }))
    }

    /// FNV-1a 64-bit hash for change detection.
    /// Fast for small inputs (config files are typically a few KB) and
    /// consistent with the hash used in `zerotrace_core::signal::attributes`.
    fn hash(s: &str) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct TestSub {
        filter: &'static str,
        called: Arc<parking_lot::Mutex<bool>>,
    }

    #[async_trait::async_trait]
    impl ConfigSubscriber for TestSub {
        fn name(&self) -> &'static str {
            "test"
        }
        fn interested(&self, c: &ConfigChange) -> bool {
            matches!(c, ConfigChange::Field { path, .. } if path.first().map(|s| s.as_str()) == Some(self.filter))
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            *self.called.lock() = true;
            Ok(Action::HotApplied)
        }
    }

    #[tokio::test]
    async fn test_dispatch() {
        let mut bus = ConfigBus::new();
        let called = Arc::new(parking_lot::Mutex::new(false));
        bus.subscribe(TestSub {
            filter: "inputs",
            called: called.clone(),
        });
        let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
        bus.dispatch(
            &ConfigChange::Field {
                path: vec!["inputs".into(), "x".into()],
                old_value: serde_json::Value::Null,
                new_value: serde_json::Value::Null,
            },
            &ctx,
        )
        .await
        .unwrap();
        assert!(*called.lock());
    }

    #[tokio::test]
    async fn test_skip_uninterested() {
        let mut bus = ConfigBus::new();
        let called = Arc::new(parking_lot::Mutex::new(false));
        bus.subscribe(TestSub {
            filter: "x",
            called: called.clone(),
        });
        let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
        bus.dispatch(
            &ConfigChange::Field {
                path: vec!["y".into()],
                old_value: serde_json::Value::Null,
                new_value: serde_json::Value::Null,
            },
            &ctx,
        )
        .await
        .unwrap();
        assert!(!*called.lock());
    }

    #[tokio::test]
    async fn test_restart_agent_shortcircuits_remaining_subscribers() {
        struct ReturnsRestartAgent;
        #[async_trait::async_trait]
        impl ConfigSubscriber for ReturnsRestartAgent {
            fn name(&self) -> &'static str {
                "restarter"
            }
            fn interested(&self, _: &ConfigChange) -> bool {
                true
            }
            async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
                Ok(Action::RestartAgent)
            }
        }
        struct ShouldNotBeCalled {
            called: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl ConfigSubscriber for ShouldNotBeCalled {
            fn name(&self) -> &'static str {
                "should_not_run"
            }
            fn interested(&self, _: &ConfigChange) -> bool {
                true
            }
            async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
                self.called.fetch_add(1, Ordering::SeqCst);
                Ok(Action::HotApplied)
            }
        }

        let called = Arc::new(AtomicUsize::new(0));
        let mut bus = ConfigBus::new();
        bus.subscribe(ReturnsRestartAgent);
        bus.subscribe(ShouldNotBeCalled {
            called: called.clone(),
        });

        let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
        let action = bus.dispatch(&ConfigChange::FullReload, &ctx).await.unwrap();
        assert_eq!(action, Action::RestartAgent);
        // Short-circuit: the second subscriber should NOT have been called
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_severity() {
        assert!(Action::RestartAgent.is_more_severe_than(&Action::RestartPipeline("p")));
        assert!(Action::RestartSelf.is_more_severe_than(&Action::HotApplied));
        assert!(!Action::HotApplied.is_more_severe_than(&Action::HotApplied));
    }

    #[test]
    fn test_config_repo_valid() {
        let dir = tempfile::tempdir().unwrap();
        let fp = dir.path().join("cfg.yaml");
        std::fs::write(&fp, "interval_ms: 100\nbatch_size: 10\n").unwrap();
        let world = World::new();
        let _repo = ConfigRepo::<serde_json::Value>::new(&fp, &world).unwrap();
        let (val, _) = world.get::<serde_json::Value>().unwrap();
        assert_eq!(val.read()["interval_ms"], 100);
    }

    #[tokio::test]
    async fn test_config_repo_watch_detects_change() {
        let dir = tempfile::tempdir().unwrap();
        let fp = dir.path().join("watch_test.yaml");
        std::fs::write(&fp, "value: 1\n").unwrap();

        let world = World::new();
        let mut repo = ConfigRepo::<serde_json::Value>::new(&fp, &world)
            .unwrap()
            .poll_interval(Duration::from_millis(50));

        // Verify initial value
        let (val, _) = world.get::<serde_json::Value>().unwrap();
        assert_eq!(val.read()["value"], 1);

        // Set up a subscriber that records the change
        let changed = Arc::new(parking_lot::Mutex::new(false));
        let changed_clone = changed.clone();

        struct WatchSub {
            called: Arc<parking_lot::Mutex<bool>>,
        }
        #[async_trait::async_trait]
        impl ConfigSubscriber for WatchSub {
            fn name(&self) -> &'static str {
                "watch_sub"
            }
            fn interested(&self, _: &ConfigChange) -> bool {
                true
            }
            async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
                *self.called.lock() = true;
                Ok(Action::HotApplied)
            }
        }

        let mut bus = ConfigBus::new();
        bus.subscribe(WatchSub {
            called: changed_clone,
        });

        let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        // Spawn the watch loop
        let _watch_handle = tokio::spawn(async move {
            let _ = repo.watch(&world, &mut bus, &ctx, shutdown_rx).await;
        });

        // Wait for the watcher to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Modify the file
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(&fp, "value: 2\n").unwrap();

        // Wait for the watcher to detect the change
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Shutdown
        let _ = shutdown_tx.send(true);
        // Give the watch task time to see the shutdown signal
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            *changed.lock(),
            "ConfigRepo::watch should detect file change and dispatch"
        );
    }
}
