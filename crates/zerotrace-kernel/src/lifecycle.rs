// Lifecycle hooks for components.
//
// Components self-register with LifecycleRegistry during construction.
//
// # Why `async_trait` here (not RPIT)?
//
// `Lifecycle` is used as a **trait object** (`Box<dyn Lifecycle>`) inside
// `LifecycleRegistry`.  RPIT (return-position impl Trait) in traits is not
// dyn-safe in current Rust.  The `async_trait` macro works around this by
// boxing the returned future, making the trait dyn-compatible.
//
// Pipeline traits (`Source`, `Processor`, `Reporter`) use RPIT because they
// are never used as trait objects — they're always instantiated with concrete
// generic types.
//
// ```rust
// use zerotrace_kernel::lifecycle::{Lifecycle, LifecycleCtx, LifecycleRegistry, Health};
// use zerotrace_kernel::world::World;
// use std::sync::Arc;
//
// struct MyComponent;
//
// #[async_trait::async_trait]
// impl Lifecycle for MyComponent {
//     fn name(&self) -> &'static str { "my_component" }
//     async fn on_start(&mut self, _ctx: &LifecycleCtx) -> zerotrace_kernel::error::Result<()> {
//         Ok(())
//     }
// }
//
// let world = Arc::new(World::new());
// let ctx = LifecycleCtx::new(world.clone(), tokio::runtime::Handle::current());
// let mut registry = LifecycleRegistry::new();
// registry.register(MyComponent);
// ```

use crate::{
    error::{Error, Result},
    world::World,
};
use std::sync::Arc;
use tokio::runtime::Handle as TokioHandle;

/// Component health status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// Operating normally.
    Healthy,
    /// Operating with reduced functionality.
    Degraded { reason: String },
    /// Not operational.
    Down { reason: String },
}

/// Context passed to lifecycle hooks.
///
/// Contains a reference-counted pointer to the World (so hooks can
/// read configuration) and a tokio runtime handle for spawning
/// background work.
#[derive(Clone)]
pub struct LifecycleCtx {
    pub world: Arc<World>,
    pub runtime: TokioHandle,
}

impl LifecycleCtx {
    pub fn new(world: Arc<World>, runtime: TokioHandle) -> Self {
        Self { world, runtime }
    }
}

/// Trait for components that have a managed lifecycle.
///
/// Components register themselves with [`LifecycleRegistry`] during
/// construction.  The registry calls `on_start` in registration order
/// and `on_stop` in reverse order.
///
/// # Note on `async_trait`
///
/// This trait uses the `async_trait` macro (not RPIT) because it must
/// be **dyn-safe**: `LifecycleRegistry` stores `Box<dyn Lifecycle>`.
/// RPIT in traits is not yet dyn-compatible.
#[async_trait::async_trait]
pub trait Lifecycle: Send + Sync + 'static {
    /// Human-readable name for logging and error messages.
    fn name(&self) -> &'static str;

    /// Called when the component should start.
    ///
    /// Default implementation is a no-op.
    async fn on_start(&mut self, _ctx: &LifecycleCtx) -> Result<()> {
        Ok(())
    }

    /// Called when the component should stop.
    ///
    /// Default implementation is a no-op.
    async fn on_stop(&mut self, _ctx: &LifecycleCtx) -> Result<()> {
        Ok(())
    }

    /// Current health status.  Default is [`Health::Healthy`].
    fn health(&self) -> Health {
        Health::Healthy
    }
}

/// Registry of lifecycle-managed components.
///
/// Components are started in registration order (FIFO) and stopped
/// in reverse order (LIFO).  If `start_all` encounters an error,
/// all previously-started components are rolled back (stopped).
///
/// The registry uses internal locking so it can be shared via `Arc`
/// and stored in the [`World`](crate::world::World).  Retrieve it
/// through [`ResLifecycle`](crate::param::ResLifecycle) or
/// [`World::get_raw::<LifecycleRegistry>()`].
///
/// # Timeouts
///
/// [`start_all_with_timeout`](Self::start_all_with_timeout) and
/// [`stop_all_with_timeout`](Self::stop_all_with_timeout) provide
/// per-component deadline enforcement.  If a component's hook
/// exceeds the deadline, it is treated as a failure (and rolled
/// back for start).
pub struct LifecycleRegistry {
    hooks: parking_lot::Mutex<Vec<Box<dyn Lifecycle>>>,
}

impl LifecycleRegistry {
    pub fn new() -> Self {
        Self {
            hooks: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Register a component.  Returns its index in the registry.
    ///
    /// Registering a component does NOT start it — call `start_all`
    /// to begin the lifecycle.
    pub fn register<L: Lifecycle>(&self, hook: L) -> usize {
        let mut hooks = self.hooks.lock();
        let id = hooks.len();
        hooks.push(Box::new(hook));
        id
    }

    /// Number of registered components.
    pub fn len(&self) -> usize {
        self.hooks.lock().len()
    }

    /// Returns `true` if no components are registered.
    pub fn is_empty(&self) -> bool {
        self.hooks.lock().is_empty()
    }

    /// Start all components in registration order.
    ///
    /// If any component's `on_start` fails, previously-started
    /// components are rolled back (their `on_stop` is called in
    /// reverse order) and the error is returned.
    ///
    /// The lock is NOT held across `on_start` / `on_stop` calls
    /// (avoiding the parking_lot-Mutex-across-await anti-pattern).
    /// Any [`register`] calls that arrive during startup are merged
    /// back when startup completes.
    pub async fn start_all(&self, ctx: &LifecycleCtx) -> Result<()> {
        let start = std::time::Instant::now();
        // Take ownership of the hooks vec so we can release the lock
        // before calling user-provided async code.
        let mut hooks = {
            let mut guard = self.hooks.lock();
            std::mem::take(&mut *guard)
        };

        for i in 0..hooks.len() {
            if let Err(e) = hooks[i].on_start(ctx).await {
                let name = hooks[i].name();
                // Rollback already-started components (no lock held)
                for j in (0..i).rev() {
                    if let Err(e) = hooks[j].on_stop(ctx).await {
                        tracing::warn!(
                            "rollback stop [{}] failed during start failure: {}",
                            hooks[j].name(),
                            e
                        );
                    }
                }
                // Merge back: any hooks registered during startup are
                // appended after the (now rolled-back) hooks.
                {
                    let mut guard = self.hooks.lock();
                    hooks.append(&mut *guard);
                    *guard = hooks;
                }
                return Err(Error::Lifecycle {
                    component: name,
                    message: e.to_string(),
                });
            }
        }

        // All started: merge back any hooks registered during startup.
        {
            let mut guard = self.hooks.lock();
            hooks.append(&mut *guard);
            *guard = hooks;
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        let m = ctx.world.metrics();
        m.lifecycle_startup_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        m.lifecycle_startup_total_ms
            .fetch_add(elapsed_ms, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Stop all components in reverse registration order (LIFO).
    ///
    /// Returns the first error encountered.  Remaining components
    /// continue to be stopped even if one fails.
    ///
    /// The lock is NOT held across `on_stop` calls.
    pub async fn stop_all(&self, ctx: &LifecycleCtx) -> Result<()> {
        let mut hooks = {
            let mut guard = self.hooks.lock();
            std::mem::take(&mut *guard)
        };

        let mut first_err: Option<Error> = None;
        for hook in hooks.iter_mut().rev() {
            if let Err(e) = hook.on_stop(ctx).await {
                if first_err.is_none() {
                    first_err = Some(Error::Lifecycle {
                        component: hook.name(),
                        message: e.to_string(),
                    });
                }
            }
        }

        // Merge back any hooks registered during shutdown.
        {
            let mut guard = self.hooks.lock();
            hooks.append(&mut *guard);
            *guard = hooks;
        }

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Start all components with per-component deadline enforcement.
    ///
    /// Each component's `on_start` must complete within `timeout`.
    /// If a component exceeds the deadline, it is treated as a failure
    /// and previously-started components are rolled back.
    ///
    /// Uses [`tokio::time::timeout`]; requires a tokio runtime context.
    pub async fn start_all_with_timeout(
        &self,
        ctx: &LifecycleCtx,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let mut hooks = {
            let mut guard = self.hooks.lock();
            std::mem::take(&mut *guard)
        };

        for i in 0..hooks.len() {
            let name = hooks[i].name();
            match tokio::time::timeout(timeout, hooks[i].on_start(ctx)).await {
                Ok(Ok(())) => {},
                Ok(Err(e)) => {
                    // Rollback
                    for j in (0..i).rev() {
                        if let Err(e) = hooks[j].on_stop(ctx).await {
                            tracing::warn!(
                                "rollback stop [{}] failed during timed start: {}",
                                hooks[j].name(),
                                e
                            );
                        }
                    }
                    {
                        let mut guard = self.hooks.lock();
                        hooks.append(&mut *guard);
                        *guard = hooks;
                    }
                    return Err(Error::Lifecycle {
                        component: name,
                        message: e.to_string(),
                    });
                },
                Err(_elapsed) => {
                    // Timeout — component hung
                    for j in (0..i).rev() {
                        if let Err(e) = hooks[j].on_stop(ctx).await {
                            tracing::warn!(
                                "rollback stop [{}] failed during timeout: {}",
                                hooks[j].name(),
                                e
                            );
                        }
                    }
                    {
                        let mut guard = self.hooks.lock();
                        hooks.append(&mut *guard);
                        *guard = hooks;
                    }
                    return Err(Error::Lifecycle {
                        component: name,
                        message: format!("start timed out after {:?}", timeout),
                    });
                },
            }
        }

        // All started
        {
            let mut guard = self.hooks.lock();
            hooks.append(&mut *guard);
            *guard = hooks;
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        let m = ctx.world.metrics();
        m.lifecycle_startup_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        m.lifecycle_startup_total_ms
            .fetch_add(elapsed_ms, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Stop all components with per-component deadline enforcement.
    ///
    /// Each component's `on_stop` must complete within `timeout`.
    /// If a component exceeds the deadline, the error is recorded
    /// but remaining components continue to be stopped.
    pub async fn stop_all_with_timeout(
        &self,
        ctx: &LifecycleCtx,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let mut hooks = {
            let mut guard = self.hooks.lock();
            std::mem::take(&mut *guard)
        };

        let mut first_err: Option<Error> = None;
        for hook in hooks.iter_mut().rev() {
            let name = hook.name();
            match tokio::time::timeout(timeout, hook.on_stop(ctx)).await {
                Ok(Ok(())) => {},
                Ok(Err(e)) =>
                    if first_err.is_none() {
                        first_err = Some(Error::Lifecycle {
                            component: name,
                            message: e.to_string(),
                        });
                    },
                Err(_elapsed) =>
                    if first_err.is_none() {
                        first_err = Some(Error::Lifecycle {
                            component: name,
                            message: format!("stop timed out after {:?}", timeout),
                        });
                    },
            }
        }

        {
            let mut guard = self.hooks.lock();
            hooks.append(&mut *guard);
            *guard = hooks;
        }

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Check health of all components.  Returns the worst status found.
    pub fn health_all(&self) -> Health {
        let hooks = self.hooks.lock();
        let mut worst = Health::Healthy;
        for hook in hooks.iter() {
            match hook.health() {
                Health::Down { reason } => {
                    return Health::Down {
                        reason: format!("{}: {}", hook.name(), reason),
                    };
                },
                Health::Degraded { reason } => {
                    worst = Health::Degraded {
                        reason: format!("{}: {}", hook.name(), reason),
                    };
                },
                Health::Healthy => {},
            }
        }
        worst
    }
}

impl Default for LifecycleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as ParkingMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestComponent {
        name: &'static str,
        start_count: Arc<AtomicUsize>,
        stop_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Lifecycle for TestComponent {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn on_start(&mut self, _ctx: &LifecycleCtx) -> Result<()> {
            self.start_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn on_stop(&mut self, _ctx: &LifecycleCtx) -> Result<()> {
            self.stop_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_start_and_stop() {
        let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
        let start_c = Arc::new(AtomicUsize::new(0));
        let stop_c = Arc::new(AtomicUsize::new(0));

        let reg = LifecycleRegistry::new();
        reg.register(TestComponent {
            name: "a",
            start_count: start_c.clone(),
            stop_count: stop_c.clone(),
        });
        reg.register(TestComponent {
            name: "b",
            start_count: start_c.clone(),
            stop_count: stop_c.clone(),
        });

        reg.start_all(&ctx).await.unwrap();
        assert_eq!(start_c.load(Ordering::SeqCst), 2);

        reg.stop_all(&ctx).await.unwrap();
        assert_eq!(stop_c.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_start_failure_rollback() {
        let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());

        let stop_c = Arc::new(AtomicUsize::new(0));

        struct FailingComponent;
        #[async_trait::async_trait]
        impl Lifecycle for FailingComponent {
            fn name(&self) -> &'static str {
                "failing"
            }
            async fn on_start(&mut self, _ctx: &LifecycleCtx) -> Result<()> {
                Err(Error::Other("injected failure".into()))
            }
        }

        let reg = LifecycleRegistry::new();
        reg.register(TestComponent {
            name: "good",
            start_count: Arc::new(AtomicUsize::new(0)),
            stop_count: stop_c.clone(),
        });
        reg.register(FailingComponent);

        let result = reg.start_all(&ctx).await;
        assert!(result.is_err());
        // The first component should have been rolled back (stopped)
        assert_eq!(stop_c.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_stop_reverse_order() {
        let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
        let seq = Arc::new(ParkingMutex::new(Vec::new()));

        struct Ordered {
            name: &'static str,
            seq: Arc<ParkingMutex<Vec<&'static str>>>,
        }
        #[async_trait::async_trait]
        impl Lifecycle for Ordered {
            fn name(&self) -> &'static str {
                self.name
            }
            async fn on_stop(&mut self, _ctx: &LifecycleCtx) -> Result<()> {
                self.seq.lock().push(self.name);
                Ok(())
            }
        }

        let reg = LifecycleRegistry::new();
        reg.register(Ordered {
            name: "first",
            seq: seq.clone(),
        });
        reg.register(Ordered {
            name: "second",
            seq: seq.clone(),
        });
        reg.register(Ordered {
            name: "third",
            seq: seq.clone(),
        });

        reg.start_all(&ctx).await.unwrap();
        reg.stop_all(&ctx).await.unwrap();
        assert_eq!(*seq.lock(), vec!["third", "second", "first"]);
    }

    #[tokio::test]
    async fn test_health_all() {
        let _ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());

        struct HealthyOne;
        #[async_trait::async_trait]
        impl Lifecycle for HealthyOne {
            fn name(&self) -> &'static str {
                "healthy"
            }
        }

        struct DegradedOne;
        #[async_trait::async_trait]
        impl Lifecycle for DegradedOne {
            fn name(&self) -> &'static str {
                "degraded"
            }
            fn health(&self) -> Health {
                Health::Degraded {
                    reason: "high latency".into(),
                }
            }
        }

        struct DownOne;
        #[async_trait::async_trait]
        impl Lifecycle for DownOne {
            fn name(&self) -> &'static str {
                "down"
            }
            fn health(&self) -> Health {
                Health::Down {
                    reason: "crashed".into(),
                }
            }
        }

        let reg = LifecycleRegistry::new();
        reg.register(HealthyOne);
        reg.register(DegradedOne);
        assert!(matches!(reg.health_all(), Health::Degraded { .. }));

        let reg2 = LifecycleRegistry::new();
        reg2.register(HealthyOne);
        reg2.register(DownOne);
        assert!(matches!(reg2.health_all(), Health::Down { .. }));
    }

    #[tokio::test]
    async fn test_default_health_is_healthy() {
        struct DefaultComponent;
        #[async_trait::async_trait]
        impl Lifecycle for DefaultComponent {
            fn name(&self) -> &'static str {
                "default"
            }
        }
        assert_eq!(DefaultComponent.health(), Health::Healthy);
    }
}
