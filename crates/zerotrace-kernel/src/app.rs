// App + Plugin — Bevy-style application bootstrap.
//
// # Pattern
//
// ```ignore
// let mut app = App::new();
// app.add_plugin(MyPlugin);
// app.add_system(Stage::Update, my_system);
// app.run(); // initializes World, runs Scheduler once
// ```
//
// `Plugin::build()` receives `&mut App` and can add systems, resources,
// bundles, and other plugins.  This is the primary extension point for
// third-party code.

use crate::{
    bundle::{Bundle, BundleSet},
    error::Result,
    param::SystemParam,
    system::{AsyncSystem, ExclusiveSystem, IntoSystem, Scheduler, Stage},
    world::World,
};
use std::sync::Arc;

// ── Plugin ───────────────────────────────────────────────────────────

/// Trait for modular extensions.  Implement this to add systems,
/// resources, and sub-plugins to the application.
pub trait Plugin: Send + Sync + 'static {
    /// Configure the application.  Called during `App::add_plugin()`.
    fn build(&self, app: &mut App);
}

// ── App ──────────────────────────────────────────────────────────────

/// Top-level application container.  Owns the `World` and the `Scheduler`.
/// Models Bevy's `App` but without the ECS layer.
///
/// The [`LifecycleRegistry`] lives in the World (registered during
/// [`World::new()`]), so there is no separate lifecycle field here.
/// Access it via `app.world.get_raw::<LifecycleRegistry>()` or
/// through the [`ResLifecycle`](crate::param::ResLifecycle) SystemParam.
pub struct App {
    pub world: World,
    pub scheduler: Scheduler,
}

impl App {
    pub fn new() -> Self {
        Self {
            world: World::new(),
            scheduler: Scheduler::new(),
        }
    }

    /// Add a plugin.  The plugin's `build()` method is called immediately
    /// with a mutable reference to this `App`.
    pub fn add_plugin<P: Plugin>(&mut self, plugin: P) -> &mut Self {
        plugin.build(self);
        self
    }

    /// Insert a resource into the World.
    ///
    /// The inner [`World::insert`] returns `true` on first insertion
    /// and `false` on overwrite (with a tracing warning).  This builder
    /// method discards the flag; call `app.world.insert(value)` directly
    /// if you need to detect double-registration.
    pub fn insert_resource<T: std::any::Any + Send + Sync>(&mut self, value: T) -> &mut Self {
        self.world.insert(value);
        self
    }

    /// Add a system to a stage.
    pub fn add_system<Param>(
        &mut self,
        stage: Stage,
        name: &'static str,
        system: impl IntoSystem<Param>,
    ) -> &mut Self
    where
        Param: SystemParam + 'static,
    {
        self.scheduler.add(stage, name, system);
        self
    }

    /// Add a startup system (runs once).
    pub fn add_startup_system<Param>(
        &mut self,
        name: &'static str,
        system: impl IntoSystem<Param>,
    ) -> &mut Self
    where
        Param: SystemParam + 'static,
    {
        self.scheduler.add_startup(name, system);
        self
    }

    /// Add an exclusive system (takes `&mut World`).
    pub fn add_exclusive_system(&mut self, system: impl ExclusiveSystem + 'static) -> &mut Self {
        self.scheduler.add_exclusive(system);
        self
    }

    /// Add an async system to a stage.
    pub fn add_async_system(&mut self, stage: Stage, system: impl AsyncSystem) -> &mut Self {
        self.scheduler.add_async(stage, system);
        self
    }

    /// Load a bundle into the World.
    pub fn load_bundle(&mut self, bundle: &dyn Bundle) -> Result<()> {
        let mut set = BundleSet::new(&self.world);
        set.load(bundle)
    }

    /// Run one full tick of the scheduler (sync systems only).
    pub fn run(&mut self) -> Result<()> {
        self.scheduler.run(&mut self.world)
    }

    /// Run one full tick including both sync and async systems.
    /// Async systems execute on the provided tokio runtime handle.
    pub async fn run_async(&mut self, handle: &tokio::runtime::Handle) -> Result<()> {
        self.scheduler.run_async(&mut self.world, handle).await
    }

    /// Access the lifecycle registry stored in the World.
    pub fn lifecycle(&self) -> Result<Arc<crate::lifecycle::LifecycleRegistry>> {
        let (lr, _) = self.world.get_raw::<crate::lifecycle::LifecycleRegistry>()?;
        Ok(lr)
    }

    /// Borrow the kernel metrics sink for observability.
    ///
    /// The returned [`Arc`] can be shared with a debug socket, Prometheus
    /// endpoint, or log reporter.  Use [`KernelMetrics::snapshot`] to
    /// dump current counters.
    pub fn metrics(&self) -> &Arc<crate::metrics::KernelMetrics> {
        self.world.metrics()
    }

    /// Replace the kernel metrics sink.  Call this to wire the framework
    /// self-observability into an external metrics collector (e.g. debug
    /// socket or Prometheus endpoint).
    pub fn set_metrics(&mut self, m: Arc<crate::metrics::KernelMetrics>) -> &mut Self {
        self.world.set_metrics(m);
        self
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{param::Res, system::Stage};

    #[derive(Debug, PartialEq)]
    struct TestConfig {
        val: u64,
    }

    struct TestPlugin;
    impl Plugin for TestPlugin {
        fn build(&self, app: &mut App) {
            app.insert_resource(TestConfig { val: 99 });
            app.add_system(
                Stage::Update,
                "check_config",
                |cfg: Res<TestConfig>| -> Result<()> {
                    assert_eq!(cfg.read().val, 99);
                    Ok(())
                },
            );
        }
    }

    #[test]
    fn test_plugin_inserts_resource_and_system() {
        let mut app = App::new();
        app.add_plugin(TestPlugin);
        assert!(app.world.contains::<TestConfig>());
    }

    #[test]
    fn test_app_run() {
        let mut app = App::new();
        app.add_plugin(TestPlugin);
        let result = app.run();
        assert!(result.is_ok());
    }

    #[test]
    fn test_app_startup() {
        #[derive(Debug, PartialEq)]
        struct Flag(u32);
        let mut app = App::new();
        app.add_startup_system("init", |mut cmd: crate::param::Commands| -> Result<()> {
            cmd.insert(Flag(1));
            Ok(())
        });
        app.run().unwrap();
        assert_eq!(app.world.get::<Flag>().unwrap().0.read().0, 1);
    }
}
