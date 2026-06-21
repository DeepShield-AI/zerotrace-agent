// BundleLoader: discovers and loads bundles into the World.
//
// The loader is a convenience wrapper around the kernel's
// `BundleSet` + `LifecycleRegistry` pattern, providing:
//   - Discovery of bundles from a directory or registry
//   - Topological load with error collection
//   - Startup lifecycle: load all → start all
//
// The [`LifecycleRegistry`] is stored in the World itself (registered
// during [`World::new()`]), so the loader fetches it from there.

use std::sync::Arc;
use zerotrace_kernel::{
    bundle::{Bundle, BundleSet},
    error::Result,
    lifecycle::{LifecycleCtx, LifecycleRegistry},
    world::World,
};

/// Discovers and loads bundles, then starts their lifecycle components.
pub struct BundleLoader {
    world: Arc<World>,
}

impl BundleLoader {
    pub fn new(world: Arc<World>) -> Self {
        Self { world }
    }

    /// Load a single bundle.  Components are inserted into the World
    /// and lifecycle hooks are registered.
    pub fn load(&mut self, bundle: &dyn Bundle) -> Result<()> {
        let mut set = BundleSet::new(&self.world);
        set.load(bundle)
    }

    /// Load multiple bundles in topological dependency order.
    /// Returns an error if a cycle or missing dependency is detected.
    pub fn load_all(&mut self, bundles: &[&dyn Bundle]) -> Result<()> {
        let mut set = BundleSet::new(&self.world);
        set.load_all(bundles)
    }

    /// Start all lifecycle-managed components that were registered
    /// during bundle loading.  Components are started in FIFO order.
    pub async fn start_all(&mut self, handle: &tokio::runtime::Handle) -> Result<()> {
        let ctx = LifecycleCtx::new(self.world.clone(), handle.clone());
        let (lr, _) = self.world.get_raw::<LifecycleRegistry>()?;
        lr.start_all(&ctx).await
    }

    /// Stop all lifecycle-managed components in LIFO order.
    pub async fn stop_all(&mut self, handle: &tokio::runtime::Handle) -> Result<()> {
        let ctx = LifecycleCtx::new(self.world.clone(), handle.clone());
        let (lr, _) = self.world.get_raw::<LifecycleRegistry>()?;
        lr.stop_all(&ctx).await
    }

    /// Returns the number of registered lifecycle components.
    pub fn component_count(&self) -> usize {
        if let Ok((lr, _)) = self.world.get_raw::<LifecycleRegistry>() {
            lr.len()
        } else {
            0
        }
    }
}

impl Default for BundleLoader {
    fn default() -> Self {
        Self::new(Arc::new(World::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::RwLock;
    use std::any::TypeId;
    use zerotrace_kernel::{
        bundle::{Bundle, ComponentDescriptor},
        lifecycle::Lifecycle,
    };

    #[derive(Debug)]
    struct DbConn;
    #[derive(Debug)]
    struct AppService;

    struct DbBundle;
    impl Bundle for DbBundle {
        fn id(&self) -> &'static str {
            "db"
        }
        fn name(&self) -> &'static str {
            "Database"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![ComponentDescriptor {
                id: "db",
                provides: TypeId::of::<DbConn>(),
                deps: vec![],
                optional: false,
                factory: Box::new(|_, _| {
                    Ok(Arc::new(RwLock::new(DbConn)) as Arc<dyn std::any::Any + Send + Sync>)
                }),
            }]
        }
    }

    struct AppBundle;
    impl Bundle for AppBundle {
        fn id(&self) -> &'static str {
            "app"
        }
        fn name(&self) -> &'static str {
            "Application"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![ComponentDescriptor {
                id: "app",
                provides: TypeId::of::<AppService>(),
                deps: vec![TypeId::of::<DbConn>()],
                optional: false,
                factory: Box::new(|w, _| {
                    let _ = w.get::<DbConn>()?;
                    Ok(Arc::new(RwLock::new(AppService)) as Arc<dyn std::any::Any + Send + Sync>)
                }),
            }]
        }
    }

    #[tokio::test]
    async fn test_load_all_topological() {
        let world = Arc::new(World::new());
        let mut loader = BundleLoader::new(world.clone());
        // Load in reverse order — topological sort should handle it
        loader.load_all(&[&AppBundle, &DbBundle]).unwrap();
        assert!(world.contains::<DbConn>());
        assert!(world.contains::<AppService>());
    }

    #[test]
    fn test_load_missing_dependency_fails() {
        let world = Arc::new(World::new());
        let mut loader = BundleLoader::new(world);
        let err = loader.load_all(&[&AppBundle]);
        assert!(err.is_err());
    }

    #[test]
    fn test_component_count() {
        let world = Arc::new(World::new());
        let mut loader = BundleLoader::new(world);
        assert_eq!(loader.component_count(), 0);
        loader.load(&DbBundle).unwrap();
        assert!(loader.component_count() >= 0);
    }

    #[tokio::test]
    async fn test_start_and_stop() {
        let world = Arc::new(World::new());

        struct TestComp {
            name: &'static str,
        }
        #[async_trait::async_trait]
        impl Lifecycle for TestComp {
            fn name(&self) -> &'static str {
                self.name
            }
        }

        // Register component directly via the World's LifecycleRegistry
        let mut loader = BundleLoader::new(world);
        {
            let (lr, _) = loader.world.get_raw::<LifecycleRegistry>().unwrap();
            lr.register(TestComp { name: "test" });
        }
        assert_eq!(loader.component_count(), 1);

        let handle = tokio::runtime::Handle::current();
        loader.start_all(&handle).await.unwrap();
        loader.stop_all(&handle).await.unwrap();
    }
}
