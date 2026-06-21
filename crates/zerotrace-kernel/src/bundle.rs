// Bundle: component grouping and loading with topological sort.

use crate::{
    error::{Error, Result},
    lifecycle::LifecycleRegistry,
    world::World,
};
use std::{
    any::{Any, TypeId},
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

/// Type-erased factory that constructs a component from World + LifecycleRegistry.
pub type ComponentFactory = Box<
    dyn Fn(&World, &LifecycleRegistry) -> std::result::Result<Arc<dyn Any + Send + Sync>, Error>
        + Send
        + Sync,
>;

pub struct ComponentDescriptor {
    pub id: &'static str,
    pub provides: TypeId,
    pub deps: Vec<TypeId>,
    pub optional: bool,
    pub factory: ComponentFactory,
}

#[derive(Debug, Clone)]
pub struct PipelineTemplate {
    pub name: String,
    pub sources: Vec<String>,
    pub processors: Vec<String>,
    pub reporters: Vec<String>,
}

pub trait Bundle: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn components(&self) -> Vec<ComponentDescriptor>;
    fn default_pipelines(&self) -> Vec<PipelineTemplate> {
        Vec::new()
    }
    fn required(&self) -> bool {
        false
    }
    fn provides(&self) -> Vec<TypeId> {
        self.components().iter().map(|c| c.provides).collect()
    }
    fn depends_on(&self) -> Vec<TypeId> {
        self.components().iter().flat_map(|c| c.deps.iter().cloned()).collect()
    }
}

pub struct BundleSet<'w> {
    world: &'w World,
}

impl<'w> BundleSet<'w> {
    pub fn new(world: &'w World) -> Self {
        Self { world }
    }

    pub fn load(&mut self, bundle: &dyn Bundle) -> Result<()> {
        let start = std::time::Instant::now();
        let (lr, _) = self.world.get_raw::<LifecycleRegistry>().map_err(|_| Error::Bundle {
            bundle_id: bundle.id(),
            message: "LifecycleRegistry not found in World".into(),
        })?;
        for desc in bundle.components() {
            for dep_tid in &desc.deps {
                if !self.world.contains_tid(*dep_tid) {
                    if desc.optional {
                        continue;
                    }
                    return Err(Error::Bundle {
                        bundle_id: bundle.id(),
                        message: format!(
                            "component [{}] requires TypeId {:?} not in World",
                            desc.id, dep_tid
                        ),
                    });
                }
            }
            let component = (desc.factory)(self.world, &lr).map_err(|e| Error::Bundle {
                bundle_id: bundle.id(),
                message: format!("factory for [{}] failed: {}", desc.id, e),
            })?;
            let rk = crate::world::ResourceKey {
                type_id: desc.provides,
                key_type_id: None,
            };
            let meta = Arc::new(crate::world::ResourceMeta::new(self.world.current_tick()));
            self.world.insert_erased(rk, component, meta);
        }
        let elapsed_ms = start.elapsed().as_millis() as u64;
        let m = self.world.metrics();
        m.bundle_load_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        m.bundle_load_total_ms
            .fetch_add(elapsed_ms, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub fn load_all(&mut self, bundles: &[&dyn Bundle]) -> Result<()> {
        if bundles.is_empty() {
            return Ok(());
        }
        let mut provider_of: HashMap<TypeId, &dyn Bundle> = HashMap::new();
        for bundle in bundles {
            for tid in bundle.provides() {
                if let Some(existing) = provider_of.insert(tid, *bundle) {
                    return Err(Error::Bundle {
                        bundle_id: bundle.id(),
                        message: format!(
                            "TypeId {:?} already provided by [{}]",
                            tid,
                            existing.id()
                        ),
                    });
                }
            }
        }
        let mut edges: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for bundle in bundles {
            edges.entry(bundle.id()).or_default();
            in_degree.entry(bundle.id()).or_insert(0);
            for dep_tid in bundle.depends_on() {
                if let Some(provider) = provider_of.get(&dep_tid) {
                    if provider.id() != bundle.id() {
                        edges.entry(provider.id()).or_default().insert(bundle.id());
                        *in_degree.entry(bundle.id()).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut queue: VecDeque<&str> = VecDeque::new();
        for bundle in bundles {
            if *in_degree.get(&bundle.id()).unwrap_or(&0) == 0 {
                queue.push_back(bundle.id());
            }
        }
        let mut sorted: Vec<&str> = Vec::new();
        while let Some(bid) = queue.pop_front() {
            sorted.push(bid);
            if let Some(deps) = edges.get(bid) {
                for &dep_id in deps {
                    let e = in_degree.get_mut(dep_id).unwrap();
                    *e = e.saturating_sub(1);
                    if *e == 0 {
                        queue.push_back(dep_id);
                    }
                }
            }
        }
        if sorted.len() != bundles.len() {
            let loaded: HashSet<&str> = sorted.iter().copied().collect();
            let unloaded: Vec<&str> =
                bundles.iter().map(|b| b.id()).filter(|id| !loaded.contains(id)).collect();
            return Err(Error::Bundle {
                bundle_id: unloaded.first().unwrap_or(&"?"),
                message: format!("cycle: {:?}", unloaded),
            });
        }
        let bundle_map: HashMap<&str, &dyn Bundle> = bundles.iter().map(|b| (b.id(), *b)).collect();
        for bid in sorted {
            self.load(bundle_map[bid])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::RwLock;

    #[derive(Debug)]
    struct DummyConfig {
        _enabled: bool,
    }
    #[derive(Debug)]
    struct DummyComponent {
        _config: Arc<RwLock<DummyConfig>>,
    }

    struct TestBundle;
    impl Bundle for TestBundle {
        fn id(&self) -> &'static str {
            "test"
        }
        fn name(&self) -> &'static str {
            "Test"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![ComponentDescriptor {
                id: "test.comp",
                provides: TypeId::of::<DummyComponent>(),
                deps: vec![TypeId::of::<DummyConfig>()],
                optional: false,
                factory: Box::new(|w, _| {
                    let (cfg, _) = w.get::<DummyConfig>()?;
                    Ok(Arc::new(RwLock::new(DummyComponent { _config: cfg })))
                }),
            }]
        }
    }

    #[derive(Debug)]
    struct AnotherComponent;
    struct AnotherBundle;
    impl Bundle for AnotherBundle {
        fn id(&self) -> &'static str {
            "another"
        }
        fn name(&self) -> &'static str {
            "Another"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![ComponentDescriptor {
                id: "another.comp",
                provides: TypeId::of::<AnotherComponent>(),
                deps: vec![],
                optional: false,
                factory: Box::new(|_, _| Ok(Arc::new(RwLock::new(AnotherComponent)))),
            }]
        }
    }

    #[derive(Debug)]
    struct DependsOnDummy {
        _dep: Arc<RwLock<DummyComponent>>,
    }
    struct DependentBundle;
    impl Bundle for DependentBundle {
        fn id(&self) -> &'static str {
            "dependent"
        }
        fn name(&self) -> &'static str {
            "Dependent"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![ComponentDescriptor {
                id: "dep.comp",
                provides: TypeId::of::<DependsOnDummy>(),
                deps: vec![TypeId::of::<DummyComponent>()],
                optional: false,
                factory: Box::new(|w, _| {
                    let (dep, _) = w.get::<DummyComponent>()?;
                    Ok(Arc::new(RwLock::new(DependsOnDummy { _dep: dep })))
                }),
            }]
        }
    }

    #[test]
    fn test_bundle_load() {
        let world = World::new();
        world.insert(DummyConfig { _enabled: true });
        let mut set = BundleSet::new(&world);
        set.load(&TestBundle).unwrap();
        assert!(world.contains::<DummyComponent>());
    }

    #[test]
    fn test_bundle_missing_dep() {
        let world = World::new();
        let mut set = BundleSet::new(&world);
        assert!(set.load(&TestBundle).is_err());
    }

    #[test]
    fn test_load_all_topological() {
        let world = World::new();
        world.insert(DummyConfig { _enabled: true });
        let mut set = BundleSet::new(&world);
        set.load_all(&[&TestBundle, &AnotherBundle, &DependentBundle]).unwrap();
        assert!(world.contains::<DummyComponent>());
        assert!(world.contains::<AnotherComponent>());
        assert!(world.contains::<DependsOnDummy>());
    }

    #[test]
    fn test_cycle_detection() {
        #[derive(Debug)]
        struct X;
        #[derive(Debug)]
        struct Y;
        struct BundleA;
        impl Bundle for BundleA {
            fn id(&self) -> &'static str {
                "a"
            }
            fn name(&self) -> &'static str {
                "A"
            }
            fn components(&self) -> Vec<ComponentDescriptor> {
                vec![ComponentDescriptor {
                    id: "x",
                    provides: TypeId::of::<X>(),
                    deps: vec![TypeId::of::<Y>()],
                    optional: false,
                    factory: Box::new(|_, _| Ok(Arc::new(RwLock::new(X)))),
                }]
            }
        }
        struct BundleB;
        impl Bundle for BundleB {
            fn id(&self) -> &'static str {
                "b"
            }
            fn name(&self) -> &'static str {
                "B"
            }
            fn components(&self) -> Vec<ComponentDescriptor> {
                vec![ComponentDescriptor {
                    id: "y",
                    provides: TypeId::of::<Y>(),
                    deps: vec![TypeId::of::<X>()],
                    optional: false,
                    factory: Box::new(|_, _| Ok(Arc::new(RwLock::new(Y)))),
                }]
            }
        }
        let world = World::new();
        let mut set = BundleSet::new(&world);
        assert!(set.load_all(&[&BundleA, &BundleB]).is_err());
    }
}
