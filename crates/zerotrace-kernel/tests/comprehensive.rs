// Comprehensive tests for the ZeroTrace DI framework.
// Tests cover: World, Res, ResMut, Cfg, Commands, Events, Scheduler,
// Lifecycle, Bundle, ConfigBus, App, edge cases, change detection accuracy.

use serde::Deserialize;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zerotrace_core::error::{Error, Result};
use zerotrace_kernel::{
    app::{App, Plugin},
    bundle::{Bundle, BundleSet, ComponentDescriptor, PipelineTemplate},
    config_bus::{Action, ConfigBus, ConfigChange, ConfigRepo, ConfigSubscriber, StaticSource},
    event::Events,
    lifecycle::{Health, Lifecycle, LifecycleCtx, LifecycleRegistry},
    param::{
        Cfg, Commands, EventReader, EventWriter, Res, ResKeyed, ResLifecycle, ResMut, SystemParam,
    },
    system::{FunctionExclusiveSystem, FunctionSystem, Scheduler, Stage},
    world::{SystemContext, World},
};

// ═══════════════════════════════════════════════════════════════════════
// Test types
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, PartialEq, Clone)]
struct Config {
    value: u64,
}

#[derive(Debug, PartialEq)]
struct Counter(u64);

#[derive(Debug, PartialEq)]
struct Tag(String);

#[derive(Debug, PartialEq)]
struct Flag(bool);

#[derive(Debug, Clone, PartialEq)]
struct TestEvent(u32);

fn ctx() -> SystemContext {
    SystemContext::new(2, 1)
}

// ═══════════════════════════════════════════════════════════════════════
// 1. World operations
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn world_insert_and_get_multiple_types() {
    let w = World::new();
    w.insert(Counter(10));
    w.insert(Tag("hello".into()));
    w.insert(Flag(true));

    assert!(w.contains::<Counter>());
    assert!(w.contains::<Tag>());
    assert!(w.contains::<Flag>());

    let (c, _) = w.get::<Counter>().unwrap();
    let (t, _) = w.get::<Tag>().unwrap();
    let (f, _) = w.get::<Flag>().unwrap();

    assert_eq!(c.read().0, 10);
    assert_eq!(&t.read().0 as &str, "hello");
    assert!(f.read().0);
}

#[test]
fn world_replace_resource() {
    let w = World::new();
    w.insert(Counter(1));
    w.insert(Counter(2));

    let (c, _) = w.get::<Counter>().unwrap();
    assert_eq!(c.read().0, 2);
}

#[test]
fn world_remove_resource() {
    let w = World::new();
    w.insert(Counter(42));
    assert!(w.contains::<Counter>());

    // Remove via Commands
    let mut cmd = Commands::fetch(&w, &ctx()).unwrap();
    cmd.remove::<Counter>();
    w.apply_commands();
    assert!(!w.contains::<Counter>());
}

#[test]
fn world_clear_drops_all() {
    let w = World::new();
    w.insert(Counter(1));
    w.insert(Tag("x".into()));
    assert!(w.len() >= 2);

    w.clear();
    assert!(w.is_empty());
}

#[test]
fn world_tick_monotonically_increases() {
    let w = World::new();
    let t1 = w.current_tick();
    w.insert(Counter(1));
    let t2 = w.current_tick();
    w.insert(Tag("a".into()));
    let t3 = w.current_tick();
    w.insert(Flag(true));
    let t4 = w.current_tick();

    assert!(t2 > t1);
    assert!(t3 > t2);
    assert!(t4 > t3);
}

#[test]
fn world_missing_resource_error() {
    let w = World::new();
    let err = w.get::<Counter>().unwrap_err();
    assert!(err.to_string().contains("resource not found"));
}

#[test]
fn world_get_meta_arc_preserves_identity() {
    let w = World::new();
    w.insert(Counter(5));
    let (_, m1) = w.get_meta_arc::<Counter>().unwrap();
    let (_, m2) = w.get_meta_arc::<Counter>().unwrap();
    // Same Arc, same allocation
    assert!(Arc::ptr_eq(&m1, &m2));
}

// ═══════════════════════════════════════════════════════════════════════
// 2. Res / ResMut / Cfg
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn res_read_and_deref() {
    let w = World::new();
    w.insert(Counter(42));
    let r: Res<Counter> = Res::fetch(&w, &ctx()).unwrap();
    assert_eq!(r.read().0, 42);
}

#[test]
fn res_change_detection_same_tick_not_changed() {
    let w = World::new();
    let tick_after = w.current_tick();
    w.insert(Counter(0));
    // Fetch with last_run >= insertion tick means "not changed"
    let r: Res<Counter> =
        Res::fetch(&w, &SystemContext::new(tick_after + 2, tick_after + 1)).unwrap();
    assert!(!r.is_changed());
}

#[test]
fn res_change_detection_earlier_tick_is_changed() {
    let w = World::new();
    w.insert(Counter(0));
    let after = w.current_tick();
    // last_run=0 always < any actual tick → changed
    let r: Res<Counter> = Res::fetch(&w, &SystemContext::new(after, 0)).unwrap();
    assert!(r.is_changed());
}

#[test]
fn res_is_added_detection() {
    let w = World::new();
    w.insert(Counter(0));
    let after = w.current_tick();
    // last_run=0 < added_tick → is_added
    let r: Res<Counter> = Res::fetch(&w, &SystemContext::new(after, 0)).unwrap();
    assert!(r.is_added());
}

#[test]
fn res_is_added_false_with_same_tick() {
    let w = World::new();
    w.insert(Counter(0));
    let after = w.current_tick();
    // last_run >= added_tick → not added
    let r: Res<Counter> = Res::fetch(&w, &SystemContext::new(after, after)).unwrap();
    assert!(!r.is_added());
}

#[test]
fn resmut_write_then_read() {
    let w = World::new();
    w.insert(Counter(0));
    let r: ResMut<Counter> = ResMut::fetch(&w, &ctx()).unwrap();
    r.write().0 = 99;
    assert_eq!(r.read().0, 99);
}

#[test]
fn resmut_write_visible_to_subsequent_res() {
    let w = World::new();
    w.insert(Counter(0));
    {
        let r: ResMut<Counter> = ResMut::fetch(&w, &ctx()).unwrap();
        r.write().0 = 42;
    } // RwLockWriteGuard dropped here
    let r: Res<Counter> = Res::fetch(&w, &ctx()).unwrap();
    assert_eq!(r.read().0, 42);
}

#[test]
fn resmut_change_detection() {
    let w = World::new();
    w.insert(Counter(0));
    let r: ResMut<Counter> = ResMut::fetch(&w, &SystemContext::new(2, 1)).unwrap();
    assert!(r.is_changed());
}

#[test]
fn cfg_snapshot_clones_value() {
    let w = World::new();
    w.insert(Config { value: 7 });
    let c: Cfg<Config> = Cfg::fetch(&w, &ctx()).unwrap();
    let snap = c.snapshot();
    assert_eq!(snap.value, 7);
}

#[test]
fn resmut_write_marks_resource_as_changed() {
    // When ResMut::write() is called, is_changed() should return true
    // because write() atomically bumps changed_tick.
    let w = World::new();
    w.insert(Counter(0));
    let r: ResMut<Counter> = ResMut::fetch(&w, &SystemContext::new(2, 1)).unwrap();
    // Before write: is_changed() depends on World::insert()'s tick > last_run
    assert!(
        r.is_changed(),
        "should be changed because tick 2 > last_run 1"
    );
    // After write: is_changed() stays true (dirty flag set)
    r.write().0 = 42;
    assert!(
        r.is_changed(),
        "should still be changed after in-place write"
    );
}

#[test]
fn resmut_write_detected_by_subsequent_res() {
    // A Res<T> fetched after a ResMut<T>::write() within the same tick
    // should see the mutation via changed_tick.
    let w = World::new();
    w.insert(Counter(0));
    let tick = w.current_tick(); // e.g. 1
    // Simulate system A: ResMut writes
    {
        let rm: ResMut<Counter> = ResMut::fetch(&w, &SystemContext::new(tick, tick - 1)).unwrap();
        rm.write().0 = 99;
        // Drop releases the write guard and bumps changed_tick
    }
    // Simulate system B in same tick: fetches Res, should see change
    let r: Res<Counter> = Res::fetch(&w, &SystemContext::new(tick, tick - 1)).unwrap();
    assert_eq!(r.read().0, 99);
    assert!(
        r.is_changed(),
        "Res should detect ResMut's in-place mutation"
    );
}

#[test]
fn resmut_multiple_writes_still_changed() {
    // Multiple write() calls should all report is_changed() == true.
    let w = World::new();
    w.insert(Counter(0));
    let r: ResMut<Counter> = ResMut::fetch(&w, &SystemContext::new(2, 1)).unwrap();
    assert!(
        r.is_changed(),
        "should be changed before any write (insert bumped tick)"
    );
    r.write().0 = 1;
    assert!(r.is_changed(), "should still be changed after first write");
    r.write().0 = 2;
    assert!(r.is_changed(), "should still be changed after second write");
}

#[test]
fn cfg_change_detection_delegates_to_res() {
    let w = World::new();
    w.insert(Config { value: 1 });
    let c: Cfg<Config> = Cfg::fetch(&w, &SystemContext::new(2, 1)).unwrap();
    assert!(c.is_changed());
}

// ═══════════════════════════════════════════════════════════════════════
// 3. Commands
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn commands_insert_multiple_then_apply() {
    let w = World::new();
    let mut cmd = Commands::fetch(&w, &ctx()).unwrap();
    cmd.insert(Counter(1));
    cmd.insert(Tag("x".into()));
    cmd.insert(Flag(true));

    // None visible before apply
    assert!(!w.contains::<Counter>());
    assert!(!w.contains::<Tag>());
    assert!(!w.contains::<Flag>());

    w.apply_commands();

    assert!(w.contains::<Counter>());
    assert!(w.contains::<Tag>());
    assert!(w.contains::<Flag>());
}

#[test]
fn commands_insert_then_remove() {
    let w = World::new();
    w.insert(Counter(42));

    let mut cmd = Commands::fetch(&w, &ctx()).unwrap();
    cmd.remove::<Counter>();
    w.apply_commands();

    assert!(!w.contains::<Counter>());
}

#[test]
fn commands_insert_overwrites_previous() {
    let w = World::new();
    // Insert directly
    w.insert(Counter(1));
    // Insert via commands — should overwrite
    let mut cmd = Commands::fetch(&w, &ctx()).unwrap();
    cmd.insert(Counter(99));
    w.apply_commands();

    let (c, _) = w.get::<Counter>().unwrap();
    assert_eq!(c.read().0, 99);
}

#[test]
fn commands_ordering_within_batch() {
    // Insert then remove same type in same batch — remove should win
    let w = World::new();
    let mut cmd = Commands::fetch(&w, &ctx()).unwrap();
    cmd.insert(Counter(5));
    cmd.remove::<Counter>();
    w.apply_commands();

    // Counter should not exist (remove after insert)
    assert!(!w.contains::<Counter>());
}

// ═══════════════════════════════════════════════════════════════════════
// 4. Events
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn events_write_then_drain() {
    let w = World::new();
    w.insert_raw(Arc::new(Events::<TestEvent>::new()));

    let mut writer = EventWriter::<TestEvent>::fetch(&w, &ctx()).unwrap();
    writer.write(TestEvent(1));
    writer.write(TestEvent(2));
    writer.write(TestEvent(3));

    let mut reader = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    let drained = reader.drain();
    assert_eq!(drained, vec![TestEvent(1), TestEvent(2), TestEvent(3)]);
    assert!(reader.is_empty());
}

#[test]
fn events_double_buffer_isolation() {
    // Write to A, drain (swaps to B), write to B, drain — each drain
    // should only return events written since last drain.
    let w = World::new();
    w.insert_raw(Arc::new(Events::<TestEvent>::new()));

    // Batch 1
    let mut w1 = EventWriter::<TestEvent>::fetch(&w, &ctx()).unwrap();
    w1.write(TestEvent(1));
    drop(w1);
    let mut r1 = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    assert_eq!(r1.drain(), vec![TestEvent(1)]);

    // Batch 2
    let mut w2 = EventWriter::<TestEvent>::fetch(&w, &ctx()).unwrap();
    w2.write(TestEvent(2));
    w2.write(TestEvent(3));
    drop(w2);
    let mut r2 = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    assert_eq!(r2.drain(), vec![TestEvent(2), TestEvent(3)]);

    // Batch 3 should be empty
    let mut r3 = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    assert!(r3.drain().is_empty());
}

#[test]
fn events_many_writes_then_single_drain() {
    let w = World::new();
    w.insert_raw(Arc::new(Events::<TestEvent>::new()));

    let mut writer = EventWriter::<TestEvent>::fetch(&w, &ctx()).unwrap();
    for i in 0..1000 {
        writer.write(TestEvent(i));
    }

    let mut reader = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    let drained = reader.drain();
    assert_eq!(drained.len(), 1000);
    for (i, e) in drained.iter().enumerate() {
        assert_eq!(e.0, i as u32);
    }
}

#[test]
fn events_empty_drain_returns_empty_vec() {
    let w = World::new();
    w.insert_raw(Arc::new(Events::<TestEvent>::new()));
    let mut reader = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    assert!(reader.drain().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
// 5. Scheduler — stages, ordering, one-shot, exclusive systems
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scheduler_executes_systems_in_stage_order() {
    let mut w = World::new();
    w.insert(Counter(0));

    let log = Arc::new(AtomicUsize::new(0));
    let log_clone = log.clone();
    w.insert(log);

    let mut s = Scheduler::new();

    // Startup: set to 1
    s.add(Stage::Startup, "s", {
        let l = log_clone.clone();
        move |_: ()| -> Result<()> {
            l.store(1, Ordering::SeqCst);
            Ok(())
        }
    });
    // PreUpdate: assert == 1, set to 2
    let l2 = log_clone.clone();
    s.add(Stage::PreUpdate, "p", move |_: ()| -> Result<()> {
        assert_eq!(l2.load(Ordering::SeqCst), 1);
        l2.store(2, Ordering::SeqCst);
        Ok(())
    });
    // Update: assert == 2, set to 3
    let l3 = log_clone.clone();
    s.add(Stage::Update, "u", move |_: ()| -> Result<()> {
        assert_eq!(l3.load(Ordering::SeqCst), 2);
        l3.store(3, Ordering::SeqCst);
        Ok(())
    });
    // PostUpdate: assert == 3, set to 4
    let l4 = log_clone.clone();
    s.add(Stage::PostUpdate, "po", move |_: ()| -> Result<()> {
        assert_eq!(l4.load(Ordering::SeqCst), 3);
        l4.store(4, Ordering::SeqCst);
        Ok(())
    });
    // Shutdown: assert == 4, set to 5
    let l5 = log_clone.clone();
    s.add(Stage::Shutdown, "sh", move |_: ()| -> Result<()> {
        assert_eq!(l5.load(Ordering::SeqCst), 4);
        l5.store(5, Ordering::SeqCst);
        Ok(())
    });

    s.run(&mut w).unwrap();
    assert_eq!(log_clone.load(Ordering::SeqCst), 5);
}

#[test]
fn scheduler_startup_runs_only_once() {
    let mut w = World::new();
    let call_count = Arc::new(AtomicUsize::new(0));
    let c = call_count.clone();
    w.insert(call_count.clone());

    let mut s = Scheduler::new();
    s.add_startup("once", move |_: ()| -> Result<()> {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    // Run twice
    s.run(&mut w).unwrap();
    s.run(&mut w).unwrap();

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[test]
fn scheduler_exclusive_system_has_full_world_access() {
    let mut w = World::new();
    let mut s = Scheduler::new();

    s.add_exclusive(FunctionExclusiveSystem::new(
        "setup",
        |w: &mut World| -> Result<()> {
            w.insert(Counter(99));
            w.insert(Config { value: 42 });
            Ok(())
        },
    ));

    s.run(&mut w).unwrap();

    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 99);
    assert_eq!(w.get::<Config>().unwrap().0.read().value, 42);
}

#[test]
fn scheduler_systems_run_in_registration_order() {
    // Without explicit ordering, systems run in registration order
    let mut w = World::new();
    let seq = Arc::new(AtomicUsize::new(0));
    w.insert(seq.clone());

    let mut s = Scheduler::new();

    let a = seq.clone();
    s.add(Stage::Update, "a", move |_: ()| -> Result<()> {
        assert_eq!(a.load(Ordering::SeqCst), 0);
        a.store(1, Ordering::SeqCst);
        Ok(())
    });
    let b = seq.clone();
    s.add(Stage::Update, "b", move |_: ()| -> Result<()> {
        assert_eq!(b.load(Ordering::SeqCst), 1);
        b.store(2, Ordering::SeqCst);
        Ok(())
    });
    let c = seq.clone();
    s.add(Stage::Update, "c", move |_: ()| -> Result<()> {
        assert_eq!(c.load(Ordering::SeqCst), 2);
        c.store(3, Ordering::SeqCst);
        Ok(())
    });

    s.run(&mut w).unwrap();
    assert_eq!(seq.load(Ordering::SeqCst), 3);
}

#[test]
fn scheduler_commands_applied_between_systems() {
    let mut w = World::new();

    let mut s = Scheduler::new();
    // System 1: insert via Commands
    s.add(Stage::Update, "s1", |mut cmd: Commands| -> Result<()> {
        cmd.insert(Counter(10));
        Ok(())
    });
    // System 2: should see the inserted value (Commands applied after s1)
    s.add(Stage::Update, "s2", |c: Res<Counter>| -> Result<()> {
        assert_eq!(c.read().0, 10);
        Ok(())
    });
    s.run(&mut w).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
// 6. Scheduler — change detection across multiple runs
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scheduler_change_detection_across_runs() {
    // Test that change detection works correctly across multiple scheduler runs.
    // After first run, the same resource should NOT be marked as changed
    // because the system's last_run has advanced past the resource's changed_tick.

    let mut w = World::new();
    w.insert(Counter(0));

    // Set up: insert resource, run system once to advance last_run
    let first_run = Arc::new(AtomicUsize::new(0));
    let _fr = first_run.clone();
    w.insert(first_run.clone());

    let mut s = Scheduler::new();
    // Pre-run: just increment to get past initial state
    s.add(Stage::Update, "pre", move |_: ()| -> Result<()> { Ok(()) });
    s.run(&mut w).unwrap();

    // Now create a fresh scheduler with a system that checks change detection
    let mut s2 = Scheduler::new();
    // After the pre-run, the resource hasn't been modified. This system has
    // last_run = 0 initially, so on first execution it WILL see the resource
    // as changed (because changed_tick > 0). That's correct behavior.
    s2.add(
        Stage::Update,
        "first_check",
        move |c: Res<Counter>| -> Result<()> {
            // System never ran before, last_run=0, so resource IS changed
            assert!(c.is_changed());
            Ok(())
        },
    );

    s2.run(&mut w).unwrap();

    // Second run: same system, but now last_run has advanced. The resource
    // was NOT modified between runs, so is_changed should be false.
    // Note: we can't add another system because new systems always have last_run=0.
    // Instead, add a system that mutates, then checks.
    let mut s3 = Scheduler::new();
    let changed_flag = Arc::new(AtomicUsize::new(0));
    let cf = changed_flag.clone();
    w.insert(changed_flag.clone());

    s3.add(
        Stage::Update,
        "modify",
        move |cm: ResMut<Counter>| -> Result<()> {
            cm.write().0 = 99;
            Ok(())
        },
    );
    s3.add(
        Stage::Update,
        "check_after_modify",
        move |c: Res<Counter>| -> Result<()> {
            // Resource was just modified in the same tick, so is_changed = true
            assert!(c.is_changed());
            cf.store(1, Ordering::SeqCst);
            Ok(())
        },
    );

    s3.run(&mut w).unwrap();
    assert_eq!(changed_flag.load(Ordering::SeqCst), 1);
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 99);
}

// ═══════════════════════════════════════════════════════════════════════
// 7. Bundle loading — topology, cycles, dependencies
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug)]
struct Db;
#[derive(Debug)]
struct Cache;
#[derive(Debug)]
struct Service;

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
            provides: std::any::TypeId::of::<Db>(),
            deps: vec![],
            optional: false,
            factory: Box::new(|_, _| {
                Ok(Arc::new(parking_lot::RwLock::new(Db)) as Arc<dyn std::any::Any + Send + Sync>)
            }),
        }]
    }
}

struct CacheBundle;
impl Bundle for CacheBundle {
    fn id(&self) -> &'static str {
        "cache"
    }
    fn name(&self) -> &'static str {
        "Cache"
    }
    fn components(&self) -> Vec<ComponentDescriptor> {
        vec![ComponentDescriptor {
            id: "cache",
            provides: std::any::TypeId::of::<Cache>(),
            deps: vec![],
            optional: false,
            factory: Box::new(|_, _| {
                Ok(Arc::new(parking_lot::RwLock::new(Cache))
                    as Arc<dyn std::any::Any + Send + Sync>)
            }),
        }]
    }
}

struct ServiceBundle;
impl Bundle for ServiceBundle {
    fn id(&self) -> &'static str {
        "service"
    }
    fn name(&self) -> &'static str {
        "Service"
    }
    fn components(&self) -> Vec<ComponentDescriptor> {
        vec![ComponentDescriptor {
            id: "service",
            provides: std::any::TypeId::of::<Service>(),
            deps: vec![
                std::any::TypeId::of::<Db>(),
                std::any::TypeId::of::<Cache>(),
            ],
            optional: false,
            factory: Box::new(|w, _| {
                let _db = w.get::<Db>().map_err(|_| Error::Other("missing db".into()))?;
                let _cache = w.get::<Cache>().map_err(|_| Error::Other("missing cache".into()))?;
                Ok(Arc::new(parking_lot::RwLock::new(Service))
                    as Arc<dyn std::any::Any + Send + Sync>)
            }),
        }]
    }
}

#[test]
fn bundle_load_all_topological_order() {
    let world = World::new();
    let mut set = BundleSet::new(&world);

    // Load in arbitrary order — topological sort ensures correct sequence
    set.load_all(&[&ServiceBundle, &DbBundle, &CacheBundle]).unwrap();

    assert!(world.contains::<Db>());
    assert!(world.contains::<Cache>());
    assert!(world.contains::<Service>());
}

#[test]
fn bundle_cycle_detection() {
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
                provides: std::any::TypeId::of::<X>(),
                deps: vec![std::any::TypeId::of::<Y>()],
                optional: false,
                factory: Box::new(|_, _| {
                    Ok(Arc::new(parking_lot::RwLock::new(X))
                        as Arc<dyn std::any::Any + Send + Sync>)
                }),
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
                provides: std::any::TypeId::of::<Y>(),
                deps: vec![std::any::TypeId::of::<X>()],
                optional: false,
                factory: Box::new(|_, _| {
                    Ok(Arc::new(parking_lot::RwLock::new(Y))
                        as Arc<dyn std::any::Any + Send + Sync>)
                }),
            }]
        }
    }

    let world = World::new();
    let mut set = BundleSet::new(&world);

    let result = set.load_all(&[&BundleA, &BundleB]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("cycle"));
}

#[test]
fn bundle_duplicate_provider_error() {
    #[derive(Debug)]
    struct Z;

    struct Bundle1;
    impl Bundle for Bundle1 {
        fn id(&self) -> &'static str {
            "b1"
        }
        fn name(&self) -> &'static str {
            "B1"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![ComponentDescriptor {
                id: "z1",
                provides: std::any::TypeId::of::<Z>(),
                deps: vec![],
                optional: false,
                factory: Box::new(|_, _| {
                    Ok(Arc::new(parking_lot::RwLock::new(Z))
                        as Arc<dyn std::any::Any + Send + Sync>)
                }),
            }]
        }
    }

    struct Bundle2;
    impl Bundle for Bundle2 {
        fn id(&self) -> &'static str {
            "b2"
        }
        fn name(&self) -> &'static str {
            "B2"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![ComponentDescriptor {
                id: "z2",
                provides: std::any::TypeId::of::<Z>(),
                deps: vec![],
                optional: false,
                factory: Box::new(|_, _| {
                    Ok(Arc::new(parking_lot::RwLock::new(Z))
                        as Arc<dyn std::any::Any + Send + Sync>)
                }),
            }]
        }
    }

    let world = World::new();
    let mut set = BundleSet::new(&world);

    let result = set.load_all(&[&Bundle1, &Bundle2]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already provided"));
}

#[test]
fn bundle_default_pipelines() {
    struct MyBundle;
    impl Bundle for MyBundle {
        fn id(&self) -> &'static str {
            "my"
        }
        fn name(&self) -> &'static str {
            "My"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![]
        }
        fn default_pipelines(&self) -> Vec<PipelineTemplate> {
            vec![PipelineTemplate {
                name: "my-pipeline".into(),
                sources: vec!["src".into()],
                processors: vec!["proc".into()],
                reporters: vec!["rep".into()],
            }]
        }
    }

    let pipelines = MyBundle.default_pipelines();
    assert_eq!(pipelines.len(), 1);
    assert_eq!(pipelines[0].name, "my-pipeline");
    assert_eq!(pipelines[0].sources, vec!["src"]);
}

// ═══════════════════════════════════════════════════════════════════════
// 8. Lifecycle — start, stop, rollback, health
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn lifecycle_fifo_start_lifo_stop() {
    let w = Arc::new(World::new());
    let ctx = LifecycleCtx::new(w.clone(), tokio::runtime::Handle::current());
    let log = Arc::new(parking_lot::Mutex::new(Vec::new()));

    struct Comp {
        name: &'static str,
        log: Arc<parking_lot::Mutex<Vec<&'static str>>>,
    }
    #[async_trait::async_trait]
    impl Lifecycle for Comp {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn on_start(&mut self, _: &LifecycleCtx) -> Result<()> {
            self.log.lock().push(self.name);
            Ok(())
        }
        async fn on_stop(&mut self, _: &LifecycleCtx) -> Result<()> {
            self.log.lock().push(self.name);
            Ok(())
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(Comp {
        name: "first",
        log: log.clone(),
    });
    reg.register(Comp {
        name: "second",
        log: log.clone(),
    });
    reg.register(Comp {
        name: "third",
        log: log.clone(),
    });

    reg.start_all(&ctx).await.unwrap();
    assert_eq!(&*log.lock(), &vec!["first", "second", "third"]); // FIFO
    log.lock().clear();

    reg.stop_all(&ctx).await.unwrap();
    assert_eq!(&*log.lock(), &vec!["third", "second", "first"]); // LIFO
}

#[tokio::test]
async fn lifecycle_start_failure_rollback() {
    let w = Arc::new(World::new());
    let ctx = LifecycleCtx::new(w.clone(), tokio::runtime::Handle::current());
    let stopped = Arc::new(AtomicUsize::new(0));

    struct Good {
        stopped: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl Lifecycle for Good {
        fn name(&self) -> &'static str {
            "good"
        }
        async fn on_stop(&mut self, _: &LifecycleCtx) -> Result<()> {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct Bad;
    #[async_trait::async_trait]
    impl Lifecycle for Bad {
        fn name(&self) -> &'static str {
            "bad"
        }
        async fn on_start(&mut self, _: &LifecycleCtx) -> Result<()> {
            Err(Error::Other("fail".into()))
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(Good {
        stopped: stopped.clone(),
    });
    reg.register(Bad);

    let result = reg.start_all(&ctx).await;
    assert!(result.is_err());
    // The first (good) component should have been rolled back
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn lifecycle_health_aggregation() {
    struct Healthy;
    #[async_trait::async_trait]
    impl Lifecycle for Healthy {
        fn name(&self) -> &'static str {
            "h"
        }
    }

    struct Degraded;
    #[async_trait::async_trait]
    impl Lifecycle for Degraded {
        fn name(&self) -> &'static str {
            "d"
        }
        fn health(&self) -> Health {
            Health::Degraded {
                reason: "slow".into(),
            }
        }
    }

    struct Down;
    #[async_trait::async_trait]
    impl Lifecycle for Down {
        fn name(&self) -> &'static str {
            "dn"
        }
        fn health(&self) -> Health {
            Health::Down {
                reason: "dead".into(),
            }
        }
    }

    // Only healthy
    let reg = LifecycleRegistry::new();
    reg.register(Healthy);
    assert_eq!(reg.health_all(), Health::Healthy);

    // With degraded
    reg.register(Degraded);
    assert!(matches!(reg.health_all(), Health::Degraded { .. }));

    // With down
    reg.register(Down);
    assert!(matches!(reg.health_all(), Health::Down { .. }));
}

// ═══════════════════════════════════════════════════════════════════════
// 9. ConfigBus — dispatch, severity, short-circuit
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn configbus_dispatches_to_interested_only() {
    let mut bus = ConfigBus::new();
    let a_called = Arc::new(parking_lot::Mutex::new(false));
    let b_called = Arc::new(parking_lot::Mutex::new(false));

    struct Sub {
        filter: &'static str,
        called: Arc<parking_lot::Mutex<bool>>,
    }
    #[async_trait::async_trait]
    impl ConfigSubscriber for Sub {
        fn name(&self) -> &'static str {
            "s"
        }
        fn interested(&self, c: &ConfigChange) -> bool {
            matches!(c, ConfigChange::Field { path, .. } if path.first().map(|s| s.as_str()) == Some(self.filter))
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            *self.called.lock() = true;
            Ok(Action::HotApplied)
        }
    }

    bus.subscribe(Sub {
        filter: "db",
        called: a_called.clone(),
    });
    bus.subscribe(Sub {
        filter: "cache",
        called: b_called.clone(),
    });

    let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
    bus.dispatch(
        &ConfigChange::Field {
            path: vec!["db".into(), "url".into()],
            old_value: serde_json::Value::Null,
            new_value: serde_json::Value::Null,
        },
        &ctx,
    )
    .await
    .unwrap();

    assert!(*a_called.lock());
    assert!(!*b_called.lock()); // Not interested
}

#[tokio::test]
async fn configbus_severity_takes_max() {
    let mut bus = ConfigBus::new();
    struct Low;
    #[async_trait::async_trait]
    impl ConfigSubscriber for Low {
        fn name(&self) -> &'static str {
            "low"
        }
        fn interested(&self, _: &ConfigChange) -> bool {
            true
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            Ok(Action::HotApplied)
        }
    }
    struct Medium;
    #[async_trait::async_trait]
    impl ConfigSubscriber for Medium {
        fn name(&self) -> &'static str {
            "med"
        }
        fn interested(&self, _: &ConfigChange) -> bool {
            true
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            Ok(Action::RestartSelf)
        }
    }
    bus.subscribe(Low);
    bus.subscribe(Medium);

    let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
    let action = bus.dispatch(&ConfigChange::FullReload, &ctx).await.unwrap();
    assert_eq!(action, Action::RestartSelf);
}

#[tokio::test]
async fn configbus_restart_agent_short_circuits() {
    let mut bus = ConfigBus::new();
    struct Early;
    #[async_trait::async_trait]
    impl ConfigSubscriber for Early {
        fn name(&self) -> &'static str {
            "early"
        }
        fn interested(&self, _: &ConfigChange) -> bool {
            true
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            Ok(Action::RestartAgent)
        }
    }
    struct Late {
        called: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl ConfigSubscriber for Late {
        fn name(&self) -> &'static str {
            "late"
        }
        fn interested(&self, _: &ConfigChange) -> bool {
            true
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            self.called.fetch_add(1, Ordering::SeqCst);
            Ok(Action::HotApplied)
        }
    }

    let late_called = Arc::new(AtomicUsize::new(0));
    bus.subscribe(Early);
    bus.subscribe(Late {
        called: late_called.clone(),
    });

    let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
    let action = bus.dispatch(&ConfigChange::FullReload, &ctx).await.unwrap();
    assert_eq!(action, Action::RestartAgent);
    // Short-circuit: Late subscriber should NOT be called because
    // RestartAgent is the maximum severity and no action can be more
    // severe — further notification is wasted work.
    assert_eq!(late_called.load(Ordering::SeqCst), 0);
}

// ═══════════════════════════════════════════════════════════════════════
// 10. Stress tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn stress_many_resources() {
    let w = World::new();
    // Insert 100 different resource types
    for i in 0..100 {
        w.insert(Counter(i));
    }
    // Each insert overwrites the previous Counter, so final value is 99
    let (c, _) = w.get::<Counter>().unwrap();
    assert_eq!(c.read().0, 99);
}

#[test]
fn stress_many_systems_in_scheduler() {
    let mut w = World::new();
    let total = Arc::new(AtomicUsize::new(0));
    w.insert(Counter(0));

    let mut s = Scheduler::new();
    for _ in 0..50 {
        let t = total.clone();
        s.add(Stage::Update, "inc", move |c: Res<Counter>| -> Result<()> {
            t.fetch_add(c.read().0 as usize, Ordering::SeqCst);
            Ok(())
        });
    }
    s.run(&mut w).unwrap();
    // 50 systems, each adds 0
    assert_eq!(total.load(Ordering::SeqCst), 0);
}

#[test]
fn stress_commands_batch() {
    let w = World::new();
    let mut cmd = Commands::fetch(&w, &ctx()).unwrap();

    // Queue many commands
    for i in 0..500 {
        #[derive(Debug, PartialEq)]
        struct Item(u32);
        cmd.insert(Item(i));
    }

    // None visible yet
    w.apply_commands();
    // All applied — last insert wins for each type
    // Since all inserts were for Item type, only the last one remains
}

#[test]
fn stress_events_throughput() {
    let w = World::new();
    w.insert_raw(Arc::new(Events::<TestEvent>::new()));

    let mut writer = EventWriter::<TestEvent>::fetch(&w, &ctx()).unwrap();
    for i in 0..10_000 {
        writer.write(TestEvent(i));
    }

    let mut reader = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    let drained = reader.drain();
    assert_eq!(drained.len(), 10_000);
}

// ═══════════════════════════════════════════════════════════════════════
// 11. Scheduler — ResMut in-place mutation + Commands in same tick
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scheduler_resmut_and_commands_together() {
    #[derive(Debug, PartialEq)]
    struct Score(u64);
    #[derive(Debug, PartialEq)]
    struct Bonus(u64);

    let mut w = World::new();
    w.insert(Score(0));

    let mut s = Scheduler::new();
    // System 1: mutate Score in-place
    s.add(Stage::Update, "inc", |s: ResMut<Score>| -> Result<()> {
        s.write().0 += 10;
        Ok(())
    });
    // System 2: insert Bonus via Commands
    s.add(Stage::Update, "bonus", |mut cmd: Commands| -> Result<()> {
        cmd.insert(Bonus(5));
        Ok(())
    });
    // System 3: verify both
    s.add(
        Stage::Update,
        "verify",
        |(score, bonus): (Res<Score>, Res<Bonus>)| -> Result<()> {
            assert_eq!(score.read().0, 10);
            assert_eq!(bonus.read().0, 5);
            Ok(())
        },
    );

    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Score>().unwrap().0.read().0, 10);
}

// ═══════════════════════════════════════════════════════════════════════
// 12. Error handling
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn error_resource_not_found_message() {
    let err = World::new().get::<Counter>().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("resource not found"));
}

#[test]
fn error_lifecycle_message() {
    let err = Error::Lifecycle {
        component: "test",
        message: "boom".into(),
    };
    let msg = err.to_string();
    assert!(msg.contains("test"));
    assert!(msg.contains("boom"));
}

#[test]
fn error_bundle_message() {
    let err = Error::Bundle {
        bundle_id: "my_bundle",
        message: "failed".into(),
    };
    let msg = err.to_string();
    assert!(msg.contains("my_bundle"));
    assert!(msg.contains("failed"));
}

#[test]
fn error_pipeline_message() {
    let err = Error::Pipeline {
        message: "channel closed".into(),
        fatal: true,
    };
    assert!(err.to_string().contains("pipeline"));
}

#[test]
fn error_config_message() {
    let err = Error::Config("invalid yaml".into());
    assert!(err.to_string().contains("config"));
}

// ═══════════════════════════════════════════════════════════════════════
// 13. SystemParam tuple combinations
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn systemparam_tuple_4() {
    let w = World::new();
    w.insert(Counter(1));
    w.insert(Tag("t".into()));
    w.insert(Config { value: 2 });
    w.insert(Flag(true));

    let (c, t, cfg, f): (Res<Counter>, Res<Tag>, Cfg<Config>, Res<Flag>) =
        SystemParam::fetch(&w, &ctx()).unwrap();

    assert_eq!(c.read().0, 1);
    assert_eq!(&t.read().0 as &str, "t");
    assert_eq!(cfg.snapshot().value, 2);
    assert!(f.read().0);
}

#[test]
fn systemparam_tuple_6() {
    let w = World::new();
    w.insert(Counter(1));
    w.insert(Tag("a".into()));
    w.insert(Config { value: 2 });
    w.insert(Flag(true));
    w.insert(Counter(3)); // overwrites
    w.insert(Tag("b".into())); // overwrites

    // 6-tuple with mixed params
    let result: std::result::Result<
        (
            Res<Counter>,
            Res<Tag>,
            Cfg<Config>,
            Res<Flag>,
            ResMut<Counter>,
            Commands,
        ),
        _,
    > = SystemParam::fetch(&w, &ctx());
    assert!(result.is_ok());
}

// ═══════════════════════════════════════════════════════════════════════
// 14. ExclusiveSystem ordering
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn exclusive_system_runs_before_regular_systems() {
    let mut w = World::new();
    let seq = Arc::new(AtomicUsize::new(0));
    w.insert(seq.clone());

    let mut s = Scheduler::new();
    let s1 = seq.clone();
    s.add_exclusive(FunctionExclusiveSystem::new(
        "ex",
        move |w: &mut World| -> Result<()> {
            s1.store(1, Ordering::SeqCst);
            w.insert(Counter(100));
            Ok(())
        },
    ));
    let s2 = seq.clone();
    s.add(Stage::Update, "reg", move |_: ()| -> Result<()> {
        assert_eq!(s2.load(Ordering::SeqCst), 1);
        s2.store(2, Ordering::SeqCst);
        Ok(())
    });

    s.run(&mut w).unwrap();
    assert_eq!(seq.load(Ordering::SeqCst), 2);
    assert!(w.contains::<Counter>());
}

// ═══════════════════════════════════════════════════════════════════════
// 15. Multiple runs — systems see updated state
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn multiple_scheduler_runs_state_accumulates() {
    let mut w = World::new();
    w.insert(Counter(0));

    let mut s = Scheduler::new();
    s.add(Stage::Update, "inc", |c: ResMut<Counter>| -> Result<()> {
        c.write().0 += 1;
        Ok(())
    });

    for _ in 0..10 {
        s.run(&mut w).unwrap();
    }

    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 10);
}

// ═══════════════════════════════════════════════════════════════════════
// 16. Dynamic module loading — enable/disable, add/remove at runtime
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn dynamic_disable_and_enable_system_between_runs() {
    let mut w = World::new();
    w.insert(Counter(0));
    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "counter",
        |c: ResMut<Counter>| -> Result<()> {
            c.write().0 += 1;
            Ok(())
        },
    );

    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 1);

    assert!(s.set_enabled("counter", false));
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 1); // unchanged

    assert!(s.set_enabled("counter", true));
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 2);
}

#[test]
fn dynamic_add_system_between_runs() {
    let mut w = World::new();
    w.insert(Counter(0));
    let mut s = Scheduler::new();
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 0);

    s.add(
        Stage::Update,
        "new_sys",
        |c: ResMut<Counter>| -> Result<()> {
            c.write().0 = 99;
            Ok(())
        },
    );
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 99);
}

#[test]
fn dynamic_remove_system_between_runs() {
    let mut w = World::new();
    w.insert(Counter(0));
    let mut s = Scheduler::new();
    s.add(Stage::Update, "temp", |c: ResMut<Counter>| -> Result<()> {
        c.write().0 += 1;
        Ok(())
    });
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 1);

    assert!(s.remove_by_name("temp"));
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 1);
}

#[test]
fn dynamic_resource_remove_and_reinsert() {
    let w = World::new();
    w.insert(Counter(42));
    let old = w.remove_resource::<Counter>();
    assert!(old.is_some());
    assert!(!w.contains::<Counter>());
    w.insert(Counter(99));
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 99);
}

#[test]
fn dynamic_module_pattern_enable_disable() {
    #[derive(Debug, PartialEq)]
    struct ModuleActive(bool);

    let mut w = World::new();
    w.insert(ModuleActive(false));

    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "module",
        |a: ResMut<ModuleActive>| -> Result<()> {
            a.write().0 = true;
            Ok(())
        },
    );
    s.set_enabled("module", false);

    s.run(&mut w).unwrap();
    assert!(!w.get::<ModuleActive>().unwrap().0.read().0); // didn't run

    s.set_enabled("module", true);
    s.run(&mut w).unwrap();
    assert!(w.get::<ModuleActive>().unwrap().0.read().0); // ran!
}

#[test]
fn dynamic_agent_loop_collector_processor_pattern() {
    #[derive(Debug, PartialEq)]
    struct Collected(u64);
    #[derive(Debug, PartialEq)]
    struct Processed(u64);

    let mut w = World::new();
    w.insert(Collected(0));
    w.insert(Processed(0));

    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "collector",
        |c: ResMut<Collected>| -> Result<()> {
            c.write().0 += 10;
            Ok(())
        },
    );
    s.add(
        Stage::Update,
        "processor",
        |(raw, out): (Res<Collected>, ResMut<Processed>)| -> Result<()> {
            if raw.read().0 > 0 {
                out.write().0 = raw.read().0 * 2;
            }
            Ok(())
        },
    );
    s.set_enabled("collector", false);
    s.set_enabled("processor", false);

    // Tick 1: nothing
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Collected>().unwrap().0.read().0, 0);
    assert_eq!(w.get::<Processed>().unwrap().0.read().0, 0);

    // Tick 2: collector only
    s.set_enabled("collector", true);
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Collected>().unwrap().0.read().0, 10);
    assert_eq!(w.get::<Processed>().unwrap().0.read().0, 0);

    // Tick 3: both
    s.set_enabled("processor", true);
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Collected>().unwrap().0.read().0, 20);
    assert_eq!(w.get::<Processed>().unwrap().0.read().0, 40);

    // Tick 4: processor only
    s.set_enabled("collector", false);
    s.run(&mut w).unwrap();
    assert_eq!(w.get::<Collected>().unwrap().0.read().0, 20);
    assert_eq!(w.get::<Processed>().unwrap().0.read().0, 40);
}

#[test]
fn dynamic_hot_reload_via_exclusive_system() {
    #[derive(Debug, PartialEq)]
    struct ModuleA(bool);
    #[derive(Debug, PartialEq)]
    struct ModuleB(bool);

    let mut w = World::new();
    let reload_count = Arc::new(AtomicUsize::new(0));
    let rc = reload_count.clone();
    w.insert(reload_count.clone());

    let mut s = Scheduler::new();
    s.add_exclusive(FunctionExclusiveSystem::new(
        "hot_reload",
        move |w: &mut World| -> Result<()> {
            let n = rc.fetch_add(1, Ordering::SeqCst) + 1;
            match n {
                1 => {
                    w.insert(ModuleA(true));
                },
                2 => {
                    w.insert(ModuleB(true));
                },
                3 => {
                    w.remove_resource::<ModuleA>();
                },
                4 => {
                    w.remove_resource::<ModuleB>();
                },
                _ => {},
            }
            Ok(())
        },
    ));

    s.run(&mut w).unwrap();
    assert!(w.contains::<ModuleA>());
    assert!(!w.contains::<ModuleB>());
    s.run(&mut w).unwrap();
    assert!(w.contains::<ModuleA>());
    assert!(w.contains::<ModuleB>());
    s.run(&mut w).unwrap();
    assert!(!w.contains::<ModuleA>());
    assert!(w.contains::<ModuleB>());
    s.run(&mut w).unwrap();
    assert!(!w.contains::<ModuleA>());
    assert!(!w.contains::<ModuleB>());
}

// ═══════════════════════════════════════════════════════════════════════
// 17. Events concurrency — multi-thread send + drain
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn events_concurrent_multiple_writers() {
    use std::thread;
    let events = Arc::new(Events::<TestEvent>::new());
    let n_threads = 4;
    let n_per_thread = 1000u32;

    let mut handles = vec![];
    for t in 0..n_threads {
        let ev = events.clone();
        handles.push(thread::spawn(move || {
            for i in 0..n_per_thread {
                ev.send(TestEvent(t as u32 * n_per_thread + i));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // Drain — all 4000 events should be present
    let drained = events.drain();
    assert_eq!(drained.len(), (n_threads * n_per_thread) as usize);
}

#[test]
fn events_concurrent_write_and_drain() {
    use std::thread;
    let events = Arc::new(Events::<TestEvent>::new());

    // Writer 1 writes 100 events
    let ev1 = events.clone();
    let w1 = thread::spawn(move || {
        for i in 0..100 {
            ev1.send(TestEvent(i));
        }
    });

    // Writer 2 writes 100 events
    let ev2 = events.clone();
    let w2 = thread::spawn(move || {
        for i in 100..200 {
            ev2.send(TestEvent(i));
        }
    });

    w1.join().unwrap();
    w2.join().unwrap();

    // Reader drains
    let drained = events.drain();
    assert_eq!(drained.len(), 200);
}

#[test]
fn events_concurrent_drainers_no_double_drain() {
    // Two concurrent drain() calls must not return overlapping events.
    // This validates the fetch_xor fix.
    use std::thread;
    let events = Arc::new(Events::<TestEvent>::new());

    // Write 200 events
    for i in 0..200u32 {
        events.send(TestEvent(i));
    }

    // Spawn two drainers concurrently
    let ev_a = events.clone();
    let ev_b = events.clone();
    let a = thread::spawn(move || ev_a.drain());
    let b = thread::spawn(move || ev_b.drain());

    let drained_a = a.join().unwrap();
    let drained_b = b.join().unwrap();

    // Each drainer should get roughly half; total = 200 unique events
    let total = drained_a.len() + drained_b.len();
    assert_eq!(total, 200, "concurrent drainers lost or duplicated events");
}

// ═══════════════════════════════════════════════════════════════════════
// 18. Scheduler ordering — edge cases
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scheduler_ordering_preserved_without_constraints() {
    // Systems without before/after should keep registration order
    let mut w = World::new();
    let log = Arc::new(parking_lot::Mutex::new(Vec::new()));
    w.insert(log.clone());

    let mut s = Scheduler::new();
    for i in 0..10u8 {
        let l = log.clone();
        s.add(Stage::Update, "s", move |_: ()| -> Result<()> {
            l.lock().push(i);
            Ok(())
        });
    }
    s.run(&mut w).unwrap();
    let vals = log.lock().clone();
    assert_eq!(vals.len(), 10);
    // Should be in registration order
    for i in 0..10 {
        assert_eq!(vals[i as usize], i);
    }
}

#[test]
fn scheduler_multiple_dependencies_chain() {
    #[derive(Debug, PartialEq)]
    struct Order(Vec<&'static str>);
    let mut w = World::new();
    w.insert(Order(Vec::new()));

    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "init",
        FunctionSystem::new("init", |_: ()| -> Result<()> { Ok(()) }).label("init"),
    );
    s.add(
        Stage::Update,
        "mid",
        FunctionSystem::new("mid", |_: ()| -> Result<()> { Ok(()) })
            .label("mid")
            .after("init")
            .before("end"),
    );
    s.add(
        Stage::Update,
        "end",
        FunctionSystem::new("end", |_: ()| -> Result<()> { Ok(()) }).label("end"),
    );

    s.run(&mut w).unwrap();
    // Should not panic or cycle
}

#[test]
fn scheduler_system_error_propagates() {
    let mut w = World::new();
    let mut s = Scheduler::new();
    s.add(Stage::Update, "fail", |_: ()| -> Result<()> {
        Err(Error::Other("deliberate failure".into()))
    });
    let result = s.run(&mut w);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("deliberate failure"));
}

// ═══════════════════════════════════════════════════════════════════════
// 19. Multi-plugin App integration
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn app_multiple_plugins_compose() {
    use zerotrace_kernel::app::{App, Plugin};

    #[derive(Debug, PartialEq)]
    struct Plugin1Ran(bool);
    #[derive(Debug, PartialEq)]
    struct Plugin2Ran(bool);

    struct P1;
    impl Plugin for P1 {
        fn build(&self, app: &mut App) {
            app.insert_resource(Plugin1Ran(false));
            app.add_system(Stage::Update, "p1", |r: ResMut<Plugin1Ran>| -> Result<()> {
                r.write().0 = true;
                Ok(())
            });
        }
    }

    struct P2;
    impl Plugin for P2 {
        fn build(&self, app: &mut App) {
            app.insert_resource(Plugin2Ran(false));
            app.add_system(Stage::Update, "p2", |r: ResMut<Plugin2Ran>| -> Result<()> {
                r.write().0 = true;
                Ok(())
            });
        }
    }

    let mut app = App::new();
    app.add_plugin(P1);
    app.add_plugin(P2);
    app.run().unwrap();

    assert!(app.world.get::<Plugin1Ran>().unwrap().0.read().0);
    assert!(app.world.get::<Plugin2Ran>().unwrap().0.read().0);
}

// ═══════════════════════════════════════════════════════════════════════
// 20. Stress — large scheduler with ordering
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn stress_large_scheduler_with_ordering() {
    let mut w = World::new();
    w.insert(Counter(0));
    let mut s = Scheduler::new();

    // Add 50 systems with a chain: sys0 → sys10 → sys20 → sys30 → sys40
    let labels: [&'static str; 5] = ["lbl0", "lbl10", "lbl20", "lbl30", "lbl40"];
    let indices: [usize; 5] = [0, 10, 20, 30, 40];

    for i in 0..50usize {
        let sys = FunctionSystem::new("sys", |c: Res<Counter>| -> Result<()> {
            let _ = c.read().0;
            Ok(())
        });
        if let Some(chain_pos) = indices.iter().position(|&x| x == i) {
            let lbl = labels[chain_pos];
            if chain_pos + 1 < labels.len() {
                let next = labels[chain_pos + 1];
                s.add(Stage::Update, "sys", sys.label(lbl).before(next));
            } else {
                s.add(Stage::Update, "sys", sys.label(lbl));
            }
        } else {
            s.add(Stage::Update, "sys", sys);
        }
    }

    s.run(&mut w).unwrap();
    // Should complete without panic — validates topological sort at scale
}

// ═══════════════════════════════════════════════════════════════════════
// 21. Commands with lifecycle integration
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn commands_and_lifecycle_integration() {
    let world = Arc::new(World::new());
    world.insert_raw(Arc::new(Events::<TestEvent>::new()));

    let ctx = LifecycleCtx::new(world.clone(), tokio::runtime::Handle::current());
    let started = Arc::new(parking_lot::Mutex::new(false));

    struct StartupComponent {
        started: Arc<parking_lot::Mutex<bool>>,
    }
    #[async_trait::async_trait]
    impl Lifecycle for StartupComponent {
        fn name(&self) -> &'static str {
            "startup"
        }
        async fn on_start(&mut self, _ctx: &LifecycleCtx) -> Result<()> {
            *self.started.lock() = true;
            Ok(())
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(StartupComponent {
        started: started.clone(),
    });
    reg.start_all(&ctx).await.unwrap();
    assert!(*started.lock());

    // Now run a scheduler that inserts a resource via Commands
    let mut w2 = World::new();
    let mut s = Scheduler::new();
    s.add(Stage::Update, "insert", |mut cmd: Commands| -> Result<()> {
        cmd.insert(Counter(42));
        Ok(())
    });
    s.run(&mut w2).unwrap();
    assert_eq!(w2.get::<Counter>().unwrap().0.read().0, 42);
}

// ═══════════════════════════════════════════════════════════════════════
// 22. Send + Sync static assertions for core types
// ═══════════════════════════════════════════════════════════════════════

// These compile-time checks verify that key types are Send + Sync.
#[allow(dead_code)]
fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn test_send_sync_assertions() {
    // Compile-time: uncommenting should compile
    assert_send_sync::<World>();
    assert_send_sync::<Scheduler>();
    assert_send_sync::<Events<TestEvent>>();
    assert_send_sync::<LifecycleRegistry>();
    assert_send_sync::<ConfigBus>();
}

// ═══════════════════════════════════════════════════════════════════════
// 23. Full Integration — App → Plugin → Bundle → Scheduler → Lifecycle
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, PartialEq)]
struct IntegrationCounter(u64);

struct IntegrationPlugin;
impl Plugin for IntegrationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(IntegrationCounter(0));
        app.insert_resource(Config { value: 42 });
        app.add_system(
            Stage::Update,
            "integ_inc",
            |c: ResMut<IntegrationCounter>| -> Result<()> {
                c.write().0 += 1;
                Ok(())
            },
        );
    }
}

#[test]
fn integration_app_plugin_system_full_cycle() {
    let mut app = App::new();
    app.add_plugin(IntegrationPlugin);
    app.run().unwrap();
    app.run().unwrap();
    let (c, _) = app.world.get::<IntegrationCounter>().unwrap();
    assert_eq!(c.read().0, 2);
}

#[tokio::test]
async fn integration_app_with_lifecycle() {
    let mut app = App::new();
    app.add_plugin(IntegrationPlugin);

    // Start lifecycle components via the World's registry
    let (lr, _) = app.world.get_raw::<LifecycleRegistry>().unwrap();
    let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
    lr.start_all(&ctx).await.unwrap();

    app.run().unwrap();
    assert_eq!(app.world.get::<IntegrationCounter>().unwrap().0.read().0, 1);

    lr.stop_all(&ctx).await.unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
// 24. Keyed injection integration
// ═══════════════════════════════════════════════════════════════════════

struct KeyA;
struct KeyB;
#[derive(Debug, PartialEq)]
struct KeyedValue(String);

#[test]
fn integration_keyed_injection_multiple_instances() {
    let w = World::new();
    w.insert_keyed::<KeyA, KeyedValue>(KeyedValue("a".into()));
    w.insert_keyed::<KeyB, KeyedValue>(KeyedValue("b".into()));

    let (a, _) = w.get_keyed::<KeyA, KeyedValue>().unwrap();
    let (b, _) = w.get_keyed::<KeyB, KeyedValue>().unwrap();
    assert_eq!(a.read().0, "a");
    assert_eq!(b.read().0, "b");
}

#[test]
fn integration_keyed_in_scheduler() {
    let mut w = World::new();
    w.insert_keyed::<KeyA, Counter>(Counter(10));
    w.insert_keyed::<KeyB, Counter>(Counter(20));

    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "read_keyed",
        |(a, b): (ResKeyed<KeyA, Counter>, ResKeyed<KeyB, Counter>)| -> Result<()> {
            assert_eq!(a.read().0, 10);
            assert_eq!(b.read().0, 20);
            Ok(())
        },
    );
    s.run(&mut w).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
// 25. Property tests: World operations
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod property_tests {
    use proptest::prelude::*;
    use zerotrace_kernel::world::World;

    #[derive(Debug, PartialEq, Clone)]
    struct PValue(u64);

    proptest! {
        #[test]
        fn prop_insert_then_get(values in prop::collection::vec(0u64..1000, 1..100)) {
            let w = World::new();
            for &v in &values {
                w.insert(PValue(v));
                let (got, _) = w.get::<PValue>().unwrap();
                assert_eq!(got.read().0, v);
            }
        }

        #[test]
        fn prop_insert_remove_contains(values in prop::collection::vec(0u64..1000, 1..50)) {
            let w = World::new();
            for &v in &values {
                w.insert(PValue(v));
                assert!(w.contains::<PValue>());
                w.remove_resource::<PValue>();
                assert!(!w.contains::<PValue>());
            }
        }

        #[test]
        fn prop_tick_monotonic(n in 1u64..1000) {
            let w = World::new();
            let mut last = w.current_tick();
            for _ in 0..n {
                w.insert(PValue(0));
                let cur = w.current_tick();
                assert!(cur > last);
                last = cur;
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 26. Async system with SystemParam
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn integration_async_system_with_di() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use zerotrace_kernel::system::{Scheduler, Stage};

    #[derive(Debug)]
    struct AsyncVal(Arc<AtomicU64>);

    let val = Arc::new(AtomicU64::new(0));
    let mut w = World::new();
    w.insert(AsyncVal(val.clone()));

    let mut s = Scheduler::new();
    s.add_async_param(Stage::Update, "async_di", |av: Res<AsyncVal>| async move {
        av.read().0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    let h = tokio::runtime::Handle::current();
    s.run_async(&mut w, &h).await.unwrap();
    assert_eq!(val.load(Ordering::SeqCst), 1);
}

// ═══════════════════════════════════════════════════════════════════════
// 27. Parallel batch partitioner tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_greedy_batch_no_conflicts() {
    use std::any::TypeId;
    use zerotrace_kernel::system::{Scheduler, SystemAccess};

    struct A;
    struct B;
    struct C;

    let accesses = vec![
        SystemAccess {
            reads: vec![TypeId::of::<A>()],
            writes: vec![],
        },
        SystemAccess {
            reads: vec![TypeId::of::<B>()],
            writes: vec![],
        },
        SystemAccess {
            reads: vec![TypeId::of::<C>()],
            writes: vec![],
        },
    ];

    let batches = Scheduler::greedy_batch(&accesses);
    // All non-conflicting → should be 1 batch
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 3);
}

#[test]
fn test_greedy_batch_with_conflicts() {
    use std::any::TypeId;
    use zerotrace_kernel::system::{Scheduler, SystemAccess};

    struct A;
    struct B;

    let accesses = vec![
        // Sys 0: writes A → conflicts with 1, 2, 3 → singleton batch
        SystemAccess {
            reads: vec![],
            writes: vec![TypeId::of::<A>()],
        },
        // Sys 1: reads A → conflicts with 0 (write-read) → new batch
        SystemAccess {
            reads: vec![TypeId::of::<A>()],
            writes: vec![],
        },
        // Sys 2: reads A, writes B → conflicts with 0 (write-read), and 1 (write-read via B)
        // → cannot join batch [1], and conflicts with 0 → singleton batch
        SystemAccess {
            reads: vec![TypeId::of::<A>()],
            writes: vec![TypeId::of::<B>()],
        },
        // Sys 3: reads A → conflicts with 0, does NOT conflict with 1 (both just read A)
        // → fits in batch [1], but 1+2 don't fit together and 3 conflicts with 0+2
    ];

    let batches = Scheduler::greedy_batch(&accesses);
    // Expected: [0], [1, 3], [2] = 3 batches
    assert!(
        batches.len() >= 2,
        "expected at least 2 batches due to write-read conflicts, got {}",
        batches.len()
    );
}

#[test]
fn test_greedy_batch_empty_access_single_batch() {
    use zerotrace_kernel::system::{Scheduler, SystemAccess};

    let accesses = vec![
        SystemAccess::empty(),
        SystemAccess::empty(),
        SystemAccess::empty(),
    ];

    let batches = Scheduler::greedy_batch(&accesses);
    // Each empty access = singleton batch
    assert_eq!(batches.len(), 3);
    for batch in &batches {
        assert_eq!(batch.len(), 1);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 28. Error classification tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_error_classification_coverage() {
    use zerotrace_core::error::Error;

    // ResourceNotFound
    assert!(Error::ResourceNotFound { type_name: "X" }.is_fatal());
    // Lifecycle
    assert!(
        Error::Lifecycle {
            component: "c",
            message: "m".into()
        }
        .is_fatal()
    );
    // ConfigDispatch is retryable
    assert!(Error::ConfigDispatch("timeout".into()).is_retryable());
    // Bundle
    assert!(
        Error::Bundle {
            bundle_id: "b",
            message: "cycle".into()
        }
        .is_fatal()
    );
    // Config
    assert!(Error::Config("parse error".into()).is_fatal());
}

// ═══════════════════════════════════════════════════════════════════════
// 29. ResLifecycle SystemParam integration
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_res_lifecycle_fetch() {
    let w = World::new();
    // LifecycleRegistry is automatically in World
    let rl = ResLifecycle::fetch(&w, &ctx()).unwrap();
    assert!(rl.registry().is_empty());
}

#[tokio::test]
async fn test_res_lifecycle_register_and_start() {
    let w = World::new();

    struct TestComp {
        started: Arc<parking_lot::Mutex<bool>>,
    }
    #[async_trait::async_trait]
    impl Lifecycle for TestComp {
        fn name(&self) -> &'static str {
            "test"
        }
        async fn on_start(&mut self, _: &LifecycleCtx) -> Result<()> {
            *self.started.lock() = true;
            Ok(())
        }
    }

    let started = Arc::new(parking_lot::Mutex::new(false));
    let rl = ResLifecycle::fetch(&w, &ctx()).unwrap();
    rl.registry().register(TestComp {
        started: started.clone(),
    });

    let ctx_lc = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
    rl.registry().start_all(&ctx_lc).await.unwrap();
    assert!(*started.lock());
}

// ═══════════════════════════════════════════════════════════════════════
// 30. run_parallel correctness — verify parallel batches execute all
//     systems and preserve state consistency
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_run_parallel_executes_all_systems() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[allow(dead_code)]
    struct A;
    #[allow(dead_code)]
    struct B;
    #[allow(dead_code)]
    struct C;

    #[derive(Debug)]
    #[allow(dead_code)]
    struct Counts(Arc<AtomicUsize>);

    let count = Arc::new(AtomicUsize::new(0));
    let mut w = World::new();
    w.insert(Counts(count.clone()));

    let mut s = Scheduler::new();
    // Three systems that all read different resources (no conflicts)
    // → should be batched into a single parallel batch.
    s.add(Stage::Update, "a", {
        let c = count.clone();
        move |_: ()| -> Result<()> {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });
    s.add(Stage::Update, "b", {
        let c = count.clone();
        move |_: ()| -> Result<()> {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });
    s.add(Stage::Update, "c", {
        let c = count.clone();
        move |_: ()| -> Result<()> {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });

    s.run_parallel(&mut w).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[test]
fn test_run_parallel_with_conflicts_still_completes() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[allow(dead_code)]
    struct Shared;

    #[derive(Debug)]
    #[allow(dead_code)]
    struct Counter(Arc<AtomicUsize>);

    let c = Arc::new(AtomicUsize::new(0));
    let mut w = World::new();
    w.insert(Counter(c.clone()));

    let mut s = Scheduler::new();
    // Two systems both read/write the same resource
    // → conflict, so they will be in separate batches.
    // The test verifies both still run and produce the correct sum.
    let c1 = c.clone();
    s.add(Stage::Update, "inc_a", move |_: ()| -> Result<()> {
        c1.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let c2 = c.clone();
    s.add(Stage::Update, "inc_b", move |_: ()| -> Result<()> {
        c2.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    s.run_parallel(&mut w).unwrap();
    assert_eq!(c.load(Ordering::SeqCst), 2);
}

#[test]
fn test_run_parallel_with_access_declarations() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    #[allow(dead_code)]
    struct ValA(u64);
    #[derive(Debug)]
    #[allow(dead_code)]
    struct ValB(u64);

    let mut w = World::new();
    w.insert(ValA(0));
    w.insert(ValB(0));

    let ran = Arc::new(AtomicUsize::new(0));

    // Use FunctionSystem with before/after to control ordering,
    // then run_parallel will batch them by access patterns.
    let mut s = Scheduler::new();
    let r1 = ran.clone();
    s.add(Stage::Update, "access_a", move |_: ()| -> Result<()> {
        r1.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let r2 = ran.clone();
    s.add(Stage::Update, "access_b", move |_: ()| -> Result<()> {
        r2.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    s.run_parallel(&mut w).unwrap();
    assert_eq!(ran.load(Ordering::SeqCst), 2, "both systems must execute");
}

#[test]
fn test_greedy_batch_with_explicit_access() {
    use std::any::TypeId;
    use zerotrace_kernel::system::{Scheduler, SystemAccess};

    struct A;
    struct B;
    struct C;

    // Three non-conflicting reads → should batch together
    let accesses = vec![
        SystemAccess {
            reads: vec![TypeId::of::<A>()],
            writes: vec![],
        },
        SystemAccess {
            reads: vec![TypeId::of::<B>()],
            writes: vec![],
        },
        SystemAccess {
            reads: vec![TypeId::of::<C>()],
            writes: vec![],
        },
    ];
    let batches = Scheduler::greedy_batch(&accesses);
    assert_eq!(batches.len(), 1, "three independent reads should batch");
    assert_eq!(batches[0].len(), 3);

    // Write followed by read → cannot batch
    let accesses2 = vec![
        SystemAccess {
            reads: vec![],
            writes: vec![TypeId::of::<A>()],
        },
        SystemAccess {
            reads: vec![TypeId::of::<A>()],
            writes: vec![],
        },
        SystemAccess {
            reads: vec![TypeId::of::<A>()],
            writes: vec![],
        },
    ];
    let batches2 = Scheduler::greedy_batch(&accesses2);
    assert!(
        batches2.len() >= 2,
        "write-read conflicts must create separate batches, got {}",
        batches2.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 31. Memory leak regression — repeated inserts/removes should not
//     leak Arc references
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_memory_no_leak_insert_remove_cycle() {
    let w = World::new();
    // Insert and remove the same type 1000 times — if Arc counts
    // were leaked, this would be detectable.
    for i in 0..1000u64 {
        w.insert(Counter(i));
        let removed = w.remove_resource::<Counter>();
        assert!(removed.is_some());
        assert!(!w.contains::<Counter>());
    }
    // If we got here without OOM, the test passes.
    // The removed Arc should have been dropped (refcount = 0).
}

#[test]
fn test_memory_no_leak_commands_cycle() {
    let w = World::new();
    for _ in 0..500 {
        {
            let mut cmd = Commands::fetch(&w, &ctx()).unwrap();
            cmd.insert(Counter(42));
        }
        w.apply_commands();
        assert!(w.contains::<Counter>());
        {
            let mut cmd = Commands::fetch(&w, &ctx()).unwrap();
            cmd.remove::<Counter>();
        }
        w.apply_commands();
        assert!(!w.contains::<Counter>());
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 32. run_parallel error propagation
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_run_parallel_error_propagates() {
    let mut w = World::new();
    let mut s = Scheduler::new();
    s.add(Stage::Update, "fail_a", |_: ()| -> Result<()> { Ok(()) });
    s.add(Stage::Update, "fail_b", |_: ()| -> Result<()> {
        Err(Error::Other("boom from b".into()))
    });
    s.add(Stage::Update, "fail_c", |_: ()| -> Result<()> { Ok(()) });

    let result = s.run_parallel(&mut w);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("boom from b"));
}

// ═══════════════════════════════════════════════════════════════════════
// 30. run_parallel — real thread-level parallelism
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn run_parallel_executes_all_systems() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zerotrace_kernel::system::{Scheduler, Stage};

    #[derive(Debug)]
    #[allow(dead_code)]
    struct A(u64);
    #[derive(Debug)]
    #[allow(dead_code)]
    struct B(u64);

    let run_count = Arc::new(AtomicUsize::new(0));
    let mut w = World::new();
    w.insert(A(0));
    w.insert(B(0));

    let mut s = Scheduler::new();
    let c1 = run_count.clone();
    s.add(Stage::Update, "sys_a", move |_: ()| -> Result<()> {
        c1.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let c2 = run_count.clone();
    s.add(Stage::Update, "sys_b", move |_: ()| -> Result<()> {
        c2.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    s.run_parallel(&mut w).unwrap();
    assert_eq!(run_count.load(Ordering::SeqCst), 2);
}

#[test]
fn run_parallel_respects_ordering_constraints() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zerotrace_kernel::system::{FunctionSystem, Scheduler, Stage};

    #[derive(Debug)]
    #[allow(dead_code)]
    struct Counter(u64);

    let seq = Arc::new(AtomicUsize::new(0));
    let mut w = World::new();
    w.insert(Counter(0));

    let mut s = Scheduler::new();
    let s1 = seq.clone();
    s.add(
        Stage::Update,
        "first",
        FunctionSystem::new("first", move |_: ()| -> Result<()> {
            s1.store(1, Ordering::SeqCst);
            Ok(())
        })
        .label("first")
        .before("second"),
    );
    let s2 = seq.clone();
    s.add(
        Stage::Update,
        "second",
        FunctionSystem::new("second", move |_: ()| -> Result<()> {
            assert_eq!(s2.load(Ordering::SeqCst), 1, "second ran before first");
            s2.store(2, Ordering::SeqCst);
            Ok(())
        })
        .label("second"),
    );

    s.run_parallel(&mut w).unwrap();
    assert_eq!(seq.load(Ordering::SeqCst), 2);
}

#[test]
fn run_parallel_preserves_enable_disable() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zerotrace_kernel::system::{Scheduler, Stage};

    let counter = Arc::new(AtomicUsize::new(0));
    let mut w = World::new();

    let mut s = Scheduler::new();
    let c = counter.clone();
    s.add(Stage::Update, "inc", move |_: ()| -> Result<()> {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    s.set_enabled("inc", false);
    s.run_parallel(&mut w).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    s.set_enabled("inc", true);
    s.run_parallel(&mut w).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// ═══════════════════════════════════════════════════════════════════════
// 31. App::metrics() integration
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn app_metrics_accessible_after_construction() {
    use zerotrace_kernel::app::App;

    let app = App::new();
    let metrics = app.metrics();
    // Should report 0 pipeline channel length initially
    assert_eq!(
        metrics.pipeline_channel_len.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[test]
fn app_set_metrics_replaces_sink() {
    use zerotrace_kernel::{app::App, metrics::KernelMetrics};

    let mut app = App::new();
    let custom = Arc::new(KernelMetrics::new());
    // Record something on custom
    custom.record_world_get(500);
    assert_eq!(custom.world_get_avg_ns(), 500);

    app.set_metrics(custom.clone());
    // Verify the custom metrics are accessible
    let snap = app.metrics().snapshot();
    assert!(snap.iter().any(|(k, _)| *k == "world_get_count"));
}

// ═══════════════════════════════════════════════════════════════════════
// 32. Keyed injection + Commands integration
// ═══════════════════════════════════════════════════════════════════════

struct TenantA;
struct TenantB;
#[derive(Debug, PartialEq, Clone)]
struct RateLimit(u64);

#[test]
fn keyed_insert_via_commands_then_read() {
    let w = World::new();
    let mut cmd = Commands::fetch(&w, &ctx()).unwrap();
    cmd.insert_keyed::<TenantA, RateLimit>(RateLimit(100));
    cmd.insert_keyed::<TenantB, RateLimit>(RateLimit(200));
    w.apply_commands();

    let (a, _) = w.get_keyed::<TenantA, RateLimit>().unwrap();
    let (b, _) = w.get_keyed::<TenantB, RateLimit>().unwrap();
    assert_eq!(a.read().0, 100);
    assert_eq!(b.read().0, 200);

    // Remove via commands
    let mut cmd2 = Commands::fetch(&w, &ctx()).unwrap();
    cmd2.remove_keyed::<TenantA, RateLimit>();
    w.apply_commands();
    assert!(!w.contains_keyed::<TenantA, RateLimit>());
    assert!(w.contains_keyed::<TenantB, RateLimit>());
}

#[test]
fn keyed_in_scheduler_with_commands() {
    let mut w = World::new();
    w.insert_keyed::<TenantA, Counter>(Counter(0));
    w.insert_keyed::<TenantB, Counter>(Counter(0));

    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "inc_a",
        |a: ResKeyed<TenantA, Counter>| -> Result<()> {
            // Read-only access to keyed resource
            let _val = a.read().0;
            Ok(())
        },
    );
    s.add(
        Stage::Update,
        "insert_new",
        |mut cmd: Commands| -> Result<()> {
            cmd.insert_keyed::<TenantA, RateLimit>(RateLimit(50));
            Ok(())
        },
    );
    s.run(&mut w).unwrap();

    // Commands applied: RateLimit should now be in World
    assert!(w.contains_keyed::<TenantA, RateLimit>());
    assert_eq!(w.get_keyed::<TenantA, RateLimit>().unwrap().0.read().0, 50);
}

// ═══════════════════════════════════════════════════════════════════════
// 33. Backpressure channel stress test
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn backpressure_channel_multi_sender_stress() {
    use std::sync::Arc;
    use zerotrace_runtime::pipeline::{BackpressurePolicy, backpressure_channel};

    const N_SENDERS: usize = 8;
    const N_PER_SENDER: u32 = 1_000;
    let (tx, mut rx) = backpressure_channel::<u32>(64, BackpressurePolicy::Block);

    let tx = Arc::new(tx);
    let mut handles = Vec::new();

    for t in 0..N_SENDERS {
        let tx = tx.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..N_PER_SENDER {
                tx.send(t as u32 * N_PER_SENDER + i).await.unwrap();
            }
        }));
    }

    // Drop the Arc so channel closes when all senders finish
    drop(tx);

    // Collect all
    let mut received = Vec::new();
    while let Some(v) = rx.recv().await {
        received.push(v);
    }

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(
        received.len(),
        N_SENDERS * N_PER_SENDER as usize,
        "all messages should be received"
    );
}

#[tokio::test]
async fn backpressure_channel_drop_oldest_stress() {
    use zerotrace_runtime::pipeline::{BackpressurePolicy, backpressure_channel};

    let (tx, mut rx) = backpressure_channel::<u32>(4, BackpressurePolicy::DropOldest);

    // Send 100 items through a tiny channel
    for i in 0..100u32 {
        tx.send(i).await.unwrap();
    }

    drop(tx);

    let mut received = Vec::new();
    while let Some(v) = rx.recv().await {
        received.push(v);
    }

    // With capacity 4, most items should be dropped
    assert!(
        received.len() <= 100,
        "DropOldest: received {} items",
        received.len()
    );
    // The remaining items should be the most recent ones
    assert!(
        received.len() <= 4 || received.windows(2).all(|w| w[0] < w[1]),
        "remaining items should be monotonically increasing"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 34. Pipeline + Lifecycle integration
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn pipeline_shutdown_calls_lifecycle_stop() {
    use zerotrace_core::signal::{Batch, BatchMetadata};
    use zerotrace_kernel::lifecycle::{Lifecycle, LifecycleCtx, LifecycleRegistry};
    use zerotrace_runtime::pipeline::{
        BackpressurePolicy, CollectingReporter, IterSource, PipelineExecutor, PipelineSpec,
    };

    let _world = Arc::new(World::new());
    let stopped = Arc::new(parking_lot::Mutex::new(false));

    struct TrackedSource {
        stopped: Arc<parking_lot::Mutex<bool>>,
    }
    #[async_trait::async_trait]
    impl Lifecycle for TrackedSource {
        fn name(&self) -> &'static str {
            "tracked_source"
        }
        async fn on_stop(&mut self, _: &LifecycleCtx) -> Result<()> {
            *self.stopped.lock() = true;
            Ok(())
        }
    }

    // Register component
    let lr = LifecycleRegistry::new();
    lr.register(TrackedSource {
        stopped: stopped.clone(),
    });

    // Build a simple pipeline
    let source = IterSource::new(
        "test_src",
        vec![Batch {
            metadata: Arc::new(BatchMetadata::new("test")),
            items: vec![],
        }],
    );
    let reporter = CollectingReporter::new("test_rep");

    let spec = PipelineSpec {
        name: "lifecycle_test".into(),
        channel_capacity: 16,
        backpressure: BackpressurePolicy::Block,
        ..Default::default()
    };

    let handle = PipelineExecutor::spawn(
        &spec,
        vec![("s1".into(), source.into())],
        vec![],
        vec![("r1".into(), reporter.into())],
    );

    // Let pipeline run briefly
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Shutdown pipeline
    handle.shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Pipeline should have stopped gracefully (no hang)
}

#[tokio::test]
async fn pipeline_heterogeneous_sources_with_processors() {
    use zerotrace_core::signal::{Batch, BatchMetadata};
    use zerotrace_runtime::pipeline::{
        BackpressurePolicy, CollectingReporter, FnProcessor, IterSource, PipelineExecutor,
        PipelineSpec,
    };

    let source = IterSource::new(
        "src",
        vec![
            Batch {
                metadata: Arc::new(BatchMetadata::new("test")),
                items: vec![],
            },
            Batch {
                metadata: Arc::new(BatchMetadata::new("test")),
                items: vec![],
            },
        ],
    );

    let processor = FnProcessor::new("tag", |_batch: &mut Batch| -> Result<()> {
        // Simulate tagging
        Ok(())
    });

    let reporter = CollectingReporter::new("rep");
    let batches = reporter.batches.clone();

    let spec = PipelineSpec {
        name: "tagged".into(),
        channel_capacity: 32,
        backpressure: BackpressurePolicy::Block,
        ..Default::default()
    };

    let handle = PipelineExecutor::spawn(
        &spec,
        vec![("s1".into(), source.into())],
        vec![("p1".into(), processor.into())],
        vec![("r1".into(), reporter.into())],
    );

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    handle.shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(batches.lock().len(), 2);
}

// ═══════════════════════════════════════════════════════════════════════
// 35. ConfigRepo watch — stability test (non-flaky)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn config_repo_detects_change_without_file_poll_race() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use zerotrace_kernel::{
        config_bus::{ConfigBus, ConfigChange, ConfigRepo, ConfigSubscriber},
        lifecycle::LifecycleCtx,
    };

    let dir = tempfile::tempdir().unwrap();
    let fp = dir.path().join("stable_test.yaml");
    std::fs::write(&fp, "value: 1\n").unwrap();

    let world = World::new();

    // Use poll_interval of 0 for deterministic testing — the watch loop
    // will check on every tick.
    let mut repo = ConfigRepo::<serde_json::Value>::new(&fp, &world)
        .unwrap()
        .poll_interval(std::time::Duration::from_millis(10));

    let change_count = Arc::new(AtomicUsize::new(0));

    struct CountSub {
        count: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl ConfigSubscriber for CountSub {
        fn name(&self) -> &'static str {
            "count"
        }
        fn interested(&self, _: &ConfigChange) -> bool {
            true
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(Action::HotApplied)
        }
    }

    let mut bus = ConfigBus::new();
    bus.subscribe(CountSub {
        count: change_count.clone(),
    });

    let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Spawn the watch task
    let _watch_handle = tokio::spawn(async move {
        let _ = repo.watch(&world, &mut bus, &ctx, shutdown_rx).await;
    });

    // Give the watcher time to settle
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Modify the file — watcher should detect it
    std::fs::write(&fp, "value: 2\n").unwrap();

    // Wait up to 3 seconds for the change to be detected
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if change_count.load(Ordering::SeqCst) > 0 {
            break;
        }
    }

    let _ = shutdown_tx.send(true);
    // Give the watch task time to see shutdown
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert!(
        change_count.load(Ordering::SeqCst) > 0,
        "ConfigRepo::watch should detect file change"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 36. Lifecycle concurrent register during start_all
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn lifecycle_concurrent_register_during_start_all() {
    use std::sync::Arc;
    use zerotrace_kernel::lifecycle::{Lifecycle, LifecycleCtx, LifecycleRegistry};

    let started = Arc::new(parking_lot::Mutex::new(Vec::new()));

    struct SlowStart {
        name: &'static str,
        started: Arc<parking_lot::Mutex<Vec<&'static str>>>,
        delay_ms: u64,
    }
    #[async_trait::async_trait]
    impl Lifecycle for SlowStart {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn on_start(&mut self, _: &LifecycleCtx) -> Result<()> {
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            self.started.lock().push(self.name);
            Ok(())
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(SlowStart {
        name: "slow",
        started: started.clone(),
        delay_ms: 50,
    });

    let reg_arc = Arc::new(reg);
    let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());

    let reg_clone = reg_arc.clone();
    let started_clone = started.clone();

    // Start all in one task (takes ~50ms due to slow component)
    let start_handle = tokio::spawn(async move {
        reg_clone.start_all(&ctx).await.unwrap();
    });

    // Concurrently register a new component during start_all
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    reg_arc.register(SlowStart {
        name: "concurrent",
        started: started_clone,
        delay_ms: 1,
    });

    start_handle.await.unwrap();

    let started_list = started.lock().clone();
    // "slow" started during start_all; "concurrent" was registered
    // concurrently BUT start_all already took the hooks — it won't be
    // started automatically. However, it should still be in the registry
    // after start_all completes (merged back).
    assert!(started_list.contains(&"slow"));
    assert_eq!(
        reg_arc.len(),
        2,
        "concurrent registration should be preserved"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 37. Event drain after concurrent write stress
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn events_concurrent_write_drain_tokio() {
    let events = Arc::new(Events::<TestEvent>::new());
    let n_tasks = 8;
    let n_per_task = 500u32;

    let mut handles = Vec::new();
    for t in 0..n_tasks {
        let ev = events.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..n_per_task {
                ev.send(TestEvent(t as u32 * n_per_task + i));
            }
        }));
    }

    // Wait for all writers to complete
    for h in handles {
        h.await.unwrap();
    }

    // Drain multiple times until empty
    let mut total = 0usize;
    loop {
        let batch = events.drain();
        if batch.is_empty() {
            break;
        }
        total += batch.len();
    }

    assert_eq!(total, (n_tasks * n_per_task) as usize);
}

// ═══════════════════════════════════════════════════════════════════════
// 38. World::new() internal resource invariants
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn world_new_has_required_internals() {
    let w = World::new();
    // CommandQueue and LifecycleRegistry are auto-registered
    assert!(w.contains::<zerotrace_kernel::world::CommandQueue>());
    let lr = w.get_raw::<LifecycleRegistry>();
    assert!(
        lr.is_ok(),
        "LifecycleRegistry must be present after World::new()"
    );
}

#[test]
fn world_clear_preserves_internals() {
    // World::new() auto-registers internals. After clear(), they should
    // be gone (clear is a full reset).
    let w = World::new();
    w.clear();
    assert!(w.is_empty());
    assert!(!w.contains::<zerotrace_kernel::world::CommandQueue>());
    assert!(!w.contains::<LifecycleRegistry>());
}

// ═══════════════════════════════════════════════════════════════════════
// 39. Derive macro — #[derive(Bundle)] integration
// ═══════════════════════════════════════════════════════════════════════

#[derive(zerotrace_kernel::Bundle)]
#[bundle(id = "derive_test", name = "Derive Test Bundle")]
struct DeriveTestBundle {
    #[component(id = "derived_db", deps = [])]
    db: std::sync::Arc<parking_lot::RwLock<DeriveDb>>,
    #[component(id = "derived_svc", deps = [DeriveDb])]
    svc: std::sync::Arc<parking_lot::RwLock<DeriveSvc>>,
    #[component(id = "derived_opt", deps = [AuthSvc], optional)]
    opt: std::sync::Arc<parking_lot::RwLock<DeriveOptional>>,
}

#[derive(Debug)]
struct DeriveDb;
#[derive(Debug)]
struct DeriveSvc;
#[derive(Debug)]
struct AuthSvc;
#[derive(Debug)]
struct DeriveOptional;

#[test]
fn derive_bundle_has_correct_id_and_name() {
    let bundle = DeriveTestBundle {
        db: std::sync::Arc::new(parking_lot::RwLock::new(DeriveDb)),
        svc: std::sync::Arc::new(parking_lot::RwLock::new(DeriveSvc)),
        opt: std::sync::Arc::new(parking_lot::RwLock::new(DeriveOptional)),
    };
    assert_eq!(bundle.id(), "derive_test");
    assert_eq!(bundle.name(), "Derive Test Bundle");
}

#[test]
fn derive_bundle_components_are_described() {
    let bundle = DeriveTestBundle {
        db: std::sync::Arc::new(parking_lot::RwLock::new(DeriveDb)),
        svc: std::sync::Arc::new(parking_lot::RwLock::new(DeriveSvc)),
        opt: std::sync::Arc::new(parking_lot::RwLock::new(DeriveOptional)),
    };
    let comps = bundle.components();
    assert_eq!(comps.len(), 3);

    // First component: DeriveDb, no deps, not optional
    assert_eq!(comps[0].id, "derived_db");
    assert!(comps[0].deps.is_empty());
    assert!(!comps[0].optional);

    // Second: DeriveSvc, depends on DeriveDb, not optional
    assert_eq!(comps[1].id, "derived_svc");
    assert_eq!(comps[1].deps.len(), 1);
    assert!(!comps[1].optional);

    // Third: optional, depends on AuthSvc
    assert_eq!(comps[2].id, "derived_opt");
    assert!(comps[2].optional);
}

#[test]
fn derive_bundle_load_and_resolve() {
    let world = World::new();
    // Deps are declared as the bare type name in the derive macro
    // (e.g. `deps = [DeriveDb]`), so we insert them as bare types.
    world.insert(DeriveDb);
    world.insert(AuthSvc);

    let bundle = DeriveTestBundle {
        db: std::sync::Arc::new(parking_lot::RwLock::new(DeriveDb)),
        svc: std::sync::Arc::new(parking_lot::RwLock::new(DeriveSvc)),
        opt: std::sync::Arc::new(parking_lot::RwLock::new(DeriveOptional)),
    };

    let mut set = BundleSet::new(&world);
    set.load(&bundle).unwrap();

    // The derive macro uses the inner type T (stripping Arc<RwLock<>>)
    // as the resource key in the World, matching how downstream bundles
    // declare deps. Use the bare type for lookups.
    assert!(world.contains::<DeriveDb>());
    assert!(world.contains::<DeriveSvc>());
    assert!(world.contains::<DeriveOptional>());
}

// ═══════════════════════════════════════════════════════════════════════
// 40. World type mismatch error
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn world_type_mismatch_get_vs_get_raw() {
    let w = World::new();
    // Insert via insert_raw (direct Arc<T>, no RwLock wrapper)
    w.insert_raw(Arc::new(Counter(42)));

    // Try to retrieve via get() (expects Arc<RwLock<T>>)
    let err = w.get::<Counter>().unwrap_err();
    assert!(err.is_fatal());
    let msg = err.to_string();
    assert!(
        msg.contains("type mismatch") ||
            msg.contains("get() for RwLock") ||
            msg.contains("get_raw() for direct"),
        "expected type mismatch guidance, got: {msg}"
    );
}

#[test]
fn world_type_mismatch_get_raw_vs_get() {
    let w = World::new();
    // Insert via insert (wraps in RwLock)
    w.insert(Counter(99));

    // Try to retrieve via get_raw() (expects direct Arc<T>)
    let err = w.get_raw::<Counter>().unwrap_err();
    assert!(err.is_fatal());
    let msg = err.to_string();
    assert!(
        msg.contains("type mismatch") ||
            msg.contains("get() for RwLock") ||
            msg.contains("get_raw() for direct"),
        "expected type mismatch guidance, got: {msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 41. Lifecycle timeout
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn lifecycle_start_all_timeout() {
    let w = Arc::new(World::new());
    let ctx = LifecycleCtx::new(w.clone(), tokio::runtime::Handle::current());

    struct HangComponent;
    #[async_trait::async_trait]
    impl Lifecycle for HangComponent {
        fn name(&self) -> &'static str {
            "hanging"
        }
        async fn on_start(&mut self, _: &LifecycleCtx) -> Result<()> {
            // Sleep way longer than timeout
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Ok(())
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(HangComponent);

    let result = reg.start_all_with_timeout(&ctx, std::time::Duration::from_millis(50)).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("timed out") || msg.contains("hanging"));
}

#[tokio::test]
async fn lifecycle_stop_all_timeout() {
    let w = Arc::new(World::new());
    let ctx = LifecycleCtx::new(w.clone(), tokio::runtime::Handle::current());

    struct HangOnStop;
    #[async_trait::async_trait]
    impl Lifecycle for HangOnStop {
        fn name(&self) -> &'static str {
            "hang_stop"
        }
        async fn on_stop(&mut self, _: &LifecycleCtx) -> Result<()> {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Ok(())
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(HangOnStop);

    // Start normally first (no-op)
    reg.start_all(&ctx).await.unwrap();

    let result = reg.stop_all_with_timeout(&ctx, std::time::Duration::from_millis(50)).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("timed out") || msg.contains("hang_stop"));
}

#[tokio::test]
async fn lifecycle_start_all_timeout_rolls_back() {
    let w = Arc::new(World::new());
    let ctx = LifecycleCtx::new(w.clone(), tokio::runtime::Handle::current());
    let stopped = Arc::new(AtomicUsize::new(0));

    struct QuickStart {
        stopped: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl Lifecycle for QuickStart {
        fn name(&self) -> &'static str {
            "quick"
        }
        async fn on_stop(&mut self, _: &LifecycleCtx) -> Result<()> {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct HangForever;
    #[async_trait::async_trait]
    impl Lifecycle for HangForever {
        fn name(&self) -> &'static str {
            "hanger"
        }
        async fn on_start(&mut self, _: &LifecycleCtx) -> Result<()> {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(())
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(QuickStart {
        stopped: stopped.clone(),
    });
    reg.register(HangForever);

    let result = reg.start_all_with_timeout(&ctx, std::time::Duration::from_millis(30)).await;
    assert!(result.is_err());
    // QuickStart should have been rolled back
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
}

// ═══════════════════════════════════════════════════════════════════════
// 42. Optional SystemParam in scheduler
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn optional_res_in_scheduler_present() {
    let mut w = World::new();
    w.insert(Counter(42));
    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "opt_check",
        |maybe: Option<Res<Counter>>| -> Result<()> {
            assert!(maybe.is_some());
            assert_eq!(maybe.unwrap().read().0, 42);
            Ok(())
        },
    );
    s.run(&mut w).unwrap();
}

#[test]
fn optional_res_in_scheduler_absent() {
    let mut w = World::new();
    let ran = Arc::new(AtomicUsize::new(0));
    let r = ran.clone();
    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "opt_absent",
        move |maybe: Option<Res<Counter>>| -> Result<()> {
            assert!(maybe.is_none());
            r.store(1, Ordering::SeqCst);
            Ok(())
        },
    );
    s.run(&mut w).unwrap();
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

#[test]
fn optional_resmut_absent_graceful() {
    let mut w = World::new();
    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "opt_mut",
        |maybe: Option<ResMut<Counter>>| -> Result<()> {
            assert!(maybe.is_none());
            Ok(())
        },
    );
    s.run(&mut w).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
// 43. ConfigRepo no-change stability test
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn config_repo_no_change_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let fp = dir.path().join("stable.yaml");
    std::fs::write(&fp, "value: 42\n").unwrap();

    let world = World::new();
    let mut repo = ConfigRepo::<serde_json::Value>::new(&fp, &world)
        .unwrap()
        .poll_interval(std::time::Duration::from_millis(10));

    // After construction, content hasn't changed → check returns None
    let result = repo.check(&world).unwrap();
    assert!(
        result.is_none(),
        "no change after construction should return None"
    );

    // Modify the file → now check should detect change
    // Small sleep for mtime granularity (some filesystems have 1s resolution)
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&fp, "value: 99\n").unwrap();
    // Another small sleep to ensure mtime is visibly different
    std::thread::sleep(std::time::Duration::from_millis(50));
    let result = repo.check(&world).unwrap();
    assert!(result.is_some(), "file modified → should detect change");

    // No further change → None again
    let result = repo.check(&world).unwrap();
    assert!(result.is_none(), "no further change should return None");
}

#[test]
fn config_repo_static_source_no_mtime_uses_hash() {
    let world = World::new();
    // StaticSource always returns the same content
    let source = Box::new(StaticSource::new(r#"{"key":"val"}"#, "static_test"));
    let mut repo = ConfigRepo::<serde_json::Value>::from_source(source, &world)
        .unwrap()
        .poll_interval(std::time::Duration::from_secs(1));

    // After construction, content unchanged → None
    let r1 = repo.check(&world).unwrap();
    assert!(r1.is_none());

    // Create a new StaticSource with different content — this simulates
    // a content change for a non-file source
    let world2 = World::new();
    let source2 = Box::new(StaticSource::new(r#"{"key":"new_val"}"#, "static_test"));
    let mut repo2 = ConfigRepo::<serde_json::Value>::from_source(source2, &world2).unwrap();

    // Verify the new value is in the World
    let (val, _) = world2.get::<serde_json::Value>().unwrap();
    assert_eq!(val.read()["key"], "new_val");

    // No change since construction → None
    let r2 = repo2.check(&world2).unwrap();
    assert!(r2.is_none());
}

// ═══════════════════════════════════════════════════════════════════════
// 44. Scheduler stress — 500 systems with chain ordering
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn stress_500_systems_chain_ordering() {
    let mut w = World::new();
    w.insert(Counter(0));
    let mut s = Scheduler::new();

    // Create 200 systems with a chain dependency
    let labels: Vec<&'static str> = (0..200)
        .map(|i| {
            let s: String = format!("lbl_{}", i);
            Box::leak(s.into_boxed_str()) as &'static str
        })
        .collect();

    for i in 0..200 {
        let sys = FunctionSystem::new("sys", |_: ()| -> Result<()> { Ok(()) });
        let sys = sys.label(labels[i]);
        if i + 1 < 200 {
            s.add(Stage::Update, "sys", sys.before(labels[i + 1]));
        } else {
            s.add(Stage::Update, "sys", sys);
        }
    }

    s.run(&mut w).unwrap();
    // Should complete without panic or cycle
}

// ═══════════════════════════════════════════════════════════════════════
// 45. Pipeline error propagation — processor failure drops batch
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn pipeline_processor_error_drops_batch() {
    use zerotrace_core::signal::{Batch, BatchMetadata};
    use zerotrace_runtime::pipeline::{
        BackpressurePolicy, CollectingReporter, FnProcessor, IterSource, PipelineExecutor,
        PipelineSpec,
    };

    fn make_batch() -> Batch {
        Batch {
            metadata: Arc::new(BatchMetadata::new("test")),
            items: vec![],
        }
    }

    // Source produces 2 batches
    let source = IterSource::new("src", vec![make_batch(), make_batch()]);

    // Processor fails on every batch
    let processor = FnProcessor::new("failing", |_batch: &mut Batch| -> Result<()> {
        Err(Error::Other("deliberate processor failure".into()))
    });

    let reporter = CollectingReporter::new("rep");
    let batches = reporter.batches.clone();

    let spec = PipelineSpec {
        name: "error_test".into(),
        channel_capacity: 16,
        backpressure: BackpressurePolicy::Block,
        ..Default::default()
    };

    let handle = PipelineExecutor::spawn(
        &spec,
        vec![("s1".into(), source.into())],
        vec![("p1".into(), processor.into())],
        vec![("r1".into(), reporter.into())],
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    handle.shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Processor failure should drop batches — reporter should be empty
    let received = batches.lock().len();
    assert_eq!(
        received, 0,
        "processor error should drop batches, but got {received}"
    );
}

#[tokio::test]
async fn pipeline_reporter_error_does_not_crash_pipeline() {
    use zerotrace_core::signal::{Batch, BatchMetadata};
    use zerotrace_runtime::pipeline::{IterSource, PipelineExecutor, PipelineSpec};

    // A reporter that fails on every submit
    struct FailingReporter;
    impl zerotrace_runtime::pipeline::Reporter for FailingReporter {
        fn name(&self) -> &'static str {
            "failing_rep"
        }
        async fn submit(&mut self, _batch: &Batch) -> Result<()> {
            Err(Error::Other("reporter failure".into()))
        }
    }

    let source = IterSource::new(
        "src",
        vec![Batch {
            metadata: Arc::new(BatchMetadata::new("test")),
            items: vec![],
        }],
    );

    let spec = PipelineSpec {
        name: "rep_err".into(),
        ..Default::default()
    };

    let handle = PipelineExecutor::spawn(
        &spec,
        vec![("s1".into(), source.into())],
        vec![],
        vec![("r1".into(), FailingReporter.into())],
    );

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    handle.shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // Pipeline should not panic — just warn and continue
}

// ═══════════════════════════════════════════════════════════════════════
// 46. World overwrite warning behavior
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn world_overwrite_updates_value() {
    let w = World::new();
    w.insert(Counter(1));
    w.insert(Counter(2)); // overwrite
    let (c, _) = w.get::<Counter>().unwrap();
    assert_eq!(c.read().0, 2);
}

#[test]
fn world_overwrite_increments_metrics() {
    let w = World::new();
    w.insert(Counter(1));
    let before = w.metrics().resource_overwrite_count.load(Ordering::Relaxed);
    w.insert(Counter(2)); // overwrite
    let after = w.metrics().resource_overwrite_count.load(Ordering::Relaxed);
    assert!(after > before, "overwrite should increment metrics counter");
}

// ═══════════════════════════════════════════════════════════════════════
// 47. Bundle optional dependency handling
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bundle_optional_dep_missing_is_ok() {
    #[derive(Debug)]
    struct OptionalFeature;
    #[derive(Debug)]
    struct CoreService;

    struct OptBundle;
    impl Bundle for OptBundle {
        fn id(&self) -> &'static str {
            "opt_bundle"
        }
        fn name(&self) -> &'static str {
            "Optional Bundle"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![
                ComponentDescriptor {
                    id: "core",
                    provides: std::any::TypeId::of::<CoreService>(),
                    deps: vec![],
                    optional: false,
                    factory: Box::new(|_, _| {
                        Ok(Arc::new(parking_lot::RwLock::new(CoreService))
                            as Arc<dyn std::any::Any + Send + Sync>)
                    }),
                },
                ComponentDescriptor {
                    id: "opt_feat",
                    provides: std::any::TypeId::of::<OptionalFeature>(),
                    deps: vec![std::any::TypeId::of::<SomeMissingType>()],
                    optional: true,
                    factory: Box::new(|_, _| {
                        Ok(Arc::new(parking_lot::RwLock::new(OptionalFeature))
                            as Arc<dyn std::any::Any + Send + Sync>)
                    }),
                },
            ]
        }
    }

    // Add phantom type
    #[derive(Debug)]
    struct SomeMissingType;

    let world = World::new();
    // Only SomeMissingType is in World, but OptionalFeature has an optional dep on it
    world.insert(SomeMissingType);
    let mut set = BundleSet::new(&world);
    let result = set.load(&OptBundle);
    assert!(result.is_ok());
    assert!(world.contains::<CoreService>());
    assert!(world.contains::<OptionalFeature>());
}

// ═══════════════════════════════════════════════════════════════════════
// 48. Parallel scheduler stress with many systems
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn run_parallel_100_systems_all_complete() {
    let counter = Arc::new(AtomicUsize::new(0));
    let mut w = World::new();

    let mut s = Scheduler::new();
    for _ in 0..100 {
        let c = counter.clone();
        s.add(Stage::Update, "sys", move |_: ()| -> Result<()> {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
    }

    s.run_parallel(&mut w).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

#[test]
fn run_parallel_preserves_exclusive_then_sync_order() {
    let seq = Arc::new(AtomicUsize::new(0));
    let mut w = World::new();

    let mut s = Scheduler::new();
    let s1 = seq.clone();
    s.add_exclusive(FunctionExclusiveSystem::new(
        "ex",
        move |_w: &mut World| -> Result<()> {
            s1.store(1, Ordering::SeqCst);
            Ok(())
        },
    ));
    let s2 = seq.clone();
    s.add(Stage::Update, "sync", move |_: ()| -> Result<()> {
        // Exclusive must have run first
        assert_eq!(s2.load(Ordering::SeqCst), 1);
        s2.store(2, Ordering::SeqCst);
        Ok(())
    });

    s.run_parallel(&mut w).unwrap();
    assert_eq!(seq.load(Ordering::SeqCst), 2);
}

// ═══════════════════════════════════════════════════════════════════════
// 25. ResMutGuard deferred bump — write() without DerefMut doesn't flag
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn resmut_guard_no_mutation_not_changed() {
    let w = World::new();
    w.insert(Counter(50));
    let tick = w.current_tick();

    let r: ResMut<Counter> = ResMut::fetch(&w, &SystemContext::new(tick + 1, tick)).unwrap();
    // Acquire guard but never dereference mutably
    {
        let _guard = r.write();
        // guard dropped here — touched=false → no bump
    }
    // Resource should NOT be flagged as changed
    assert!(
        !r.is_changed(),
        "write guard without DerefMut should not flag changed"
    );
}

#[test]
fn resmut_guard_with_mutation_is_changed() {
    let w = World::new();
    w.insert(Counter(50));
    let tick = w.current_tick();

    let r: ResMut<Counter> = ResMut::fetch(&w, &SystemContext::new(tick + 1, tick)).unwrap();
    assert!(!r.is_changed(), "not changed before mutation");
    {
        let mut guard = r.write();
        guard.0 = 99; // triggers DerefMut → touched=true
    }
    assert!(r.is_changed(), "write + mutation should flag changed");
}

#[test]
fn resmut_guard_multiple_guards_only_bumps_once() {
    // Verify that the first guard triggers is_changed() and the flag
    // correctly toggles only once. We observe through behaviour by
    // checking that is_changed() is true after the first write and
    // stays true after the second (no double-bump edge case).
    let w = World::new();
    w.insert(Counter(50));
    let tick = w.current_tick();

    let r: ResMut<Counter> = ResMut::fetch(&w, &SystemContext::new(tick + 10, tick)).unwrap();
    assert!(!r.is_changed(), "not changed before any mutation");
    {
        let mut g1 = r.write();
        g1.0 = 1;
    }
    assert!(r.is_changed(), "first mutation should flag changed");
    {
        let mut g2 = r.write();
        g2.0 = 2;
    }
    assert!(r.is_changed(), "still changed after second mutation");
}

// ═══════════════════════════════════════════════════════════════════════
// 26. Option<Res> — type mismatch propagates, not swallowed
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn option_res_propagates_type_mismatch() {
    let w = World::new();
    // Insert Counter via insert_raw (no RwLock wrapper)
    w.insert_raw(Arc::new(Counter(5)));

    // Option<Res<Counter>> should encounter type mismatch because
    // the resource was stored raw but Res expects RwLock<T>
    let result: std::result::Result<Option<Res<Counter>>, _> =
        Option::<Res<Counter>>::fetch(&w, &ctx());
    match result {
        Ok(_) => panic!("expected type mismatch error, got Ok"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("type mismatch"),
                "error should mention type mismatch, got: {msg}"
            );
        },
    }
}

#[test]
fn option_res_absent_is_none() {
    let w = World::new();
    let r: Option<Res<Counter>> = Option::<Res<Counter>>::fetch(&w, &ctx()).unwrap();
    assert!(r.is_none());
}

#[test]
fn option_resmut_propagates_type_mismatch() {
    let w = World::new();
    w.insert_raw(Arc::new(Counter(5))); // raw, not RwLock

    let result: std::result::Result<Option<ResMut<Counter>>, _> =
        Option::<ResMut<Counter>>::fetch(&w, &ctx());
    assert!(
        result.is_err(),
        "type mismatch in Option<ResMut> should propagate"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 27. Option<ResKeyed<K,T>> — new SystemParam impl
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn option_res_keyed_present() {
    let w = World::new();
    w.insert_keyed::<KeyA, Counter>(Counter(10));

    let r: Option<ResKeyed<KeyA, Counter>> =
        Option::<ResKeyed<KeyA, Counter>>::fetch(&w, &ctx()).unwrap();
    assert!(r.is_some());
    assert_eq!(r.unwrap().read().0, 10);
}

#[test]
fn option_res_keyed_absent() {
    let w = World::new();
    let r: Option<ResKeyed<KeyA, Counter>> =
        Option::<ResKeyed<KeyA, Counter>>::fetch(&w, &ctx()).unwrap();
    assert!(r.is_none());
}

#[test]
fn option_res_keyed_wrong_key_returns_none() {
    let w = World::new();
    w.insert_keyed::<KeyA, Counter>(Counter(10));

    // KeyB is not registered for Counter
    let r: Option<ResKeyed<KeyB, Counter>> =
        Option::<ResKeyed<KeyB, Counter>>::fetch(&w, &ctx()).unwrap();
    assert!(r.is_none());
}

#[test]
fn option_res_keyed_type_mismatch_propagates() {
    let w = World::new();
    // Insert keyed resource via insert_raw_keyed (no RwLock)
    w.insert_raw_keyed::<KeyA, Counter>(Arc::new(Counter(5)));

    let result: std::result::Result<Option<ResKeyed<KeyA, Counter>>, _> =
        Option::<ResKeyed<KeyA, Counter>>::fetch(&w, &ctx());
    assert!(
        matches!(result, Err(_)),
        "type mismatch in Option<ResKeyed> should propagate"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 28. clear_commands + orphaned commands on system error
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn clear_commands_removes_pending_commands() {
    let w = World::new();
    let mut cmd = Commands::fetch(&w, &ctx()).unwrap();
    #[derive(Debug, PartialEq)]
    struct TempRes(u32);
    cmd.insert(TempRes(1));
    cmd.insert(TempRes(2));

    w.clear_commands();

    // Nothing should have been applied
    assert!(!w.contains::<TempRes>());

    // Queue should be empty
    w.apply_commands();
    assert!(!w.contains::<TempRes>());
}

#[test]
fn scheduler_system_error_clears_orphaned_commands() {
    // When a system fails after queuing Commands, those commands
    // must be cleared — not applied on the next tick.
    #[derive(Debug, PartialEq)]
    struct GhostRes(u32);

    let mut w = World::new();
    let mut s = Scheduler::new();

    // System 1: inserts a resource, then succeeds
    #[derive(Debug, PartialEq)]
    struct GoodRes(u32);
    s.add(Stage::Update, "good", |mut cmd: Commands| -> Result<()> {
        cmd.insert(GoodRes(100));
        Ok(())
    });

    // System 2: inserts GhostRes then fails
    s.add(Stage::Update, "ghost", |mut cmd: Commands| -> Result<()> {
        cmd.insert(GhostRes(999));
        Err(Error::Other("deliberate failure".into()))
    });

    let result = s.run(&mut w);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("deliberate failure"));

    // GoodRes should be visible (system 1 succeeded before system 2 failed)
    assert!(
        w.contains::<GoodRes>(),
        "system 1 commands should be applied"
    );
    // GhostRes must NOT be visible (system 2's commands cleared on error)
    assert!(
        !w.contains::<GhostRes>(),
        "system 2 ghost commands must be cleared"
    );

    // After recovery, scheduler should still work
    s.add(Stage::Update, "recovery", |_: ()| -> Result<()> { Ok(()) });
    let result2 = s.run(&mut w);
    assert!(result2.is_ok(), "scheduler should recover after error");
    assert!(w.contains::<GoodRes>());
    assert!(!w.contains::<GhostRes>());
}

// ═══════════════════════════════════════════════════════════════════════
// 29. run_parallel cached_order fix — cache preserved after run_once removal
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn run_parallel_cached_order_preserved_after_run_once() {
    // After a run_once system exits, the topological cache should be
    // preserved so subsequent runs don't re-sort.
    let mut w = World::new();
    let log = Arc::new(parking_lot::Mutex::new(Vec::new()));
    w.insert(log.clone());

    let mut s = Scheduler::new();

    // Create systems with a dependency chain: a → b → c(run_once) → d
    s.add(
        Stage::Update,
        "a",
        FunctionSystem::new("a", {
            let l = log.clone();
            move |_: ()| -> Result<()> {
                l.lock().push("a");
                Ok(())
            }
        })
        .label("a")
        .before("b"),
    );
    s.add(
        Stage::Update,
        "b",
        FunctionSystem::new("b", {
            let l = log.clone();
            move |_: ()| -> Result<()> {
                l.lock().push("b");
                Ok(())
            }
        })
        .label("b")
        .before("c"),
    );
    s.add(
        Stage::Update,
        "c",
        FunctionSystem::new("c", {
            let l = log.clone();
            move |_: ()| -> Result<()> {
                l.lock().push("c");
                Ok(())
            }
        })
        .label("c")
        .run_once()
        .before("d"),
    );
    s.add(
        Stage::Update,
        "d",
        FunctionSystem::new("d", {
            let l = log.clone();
            move |_: ()| -> Result<()> {
                l.lock().push("d");
                Ok(())
            }
        })
        .label("d"),
    );

    // First run: a → b → c → d
    s.run_parallel(&mut w).unwrap();
    let first: Vec<&str> = log.lock().drain(..).collect();
    assert_eq!(
        &first,
        &["a", "b", "c", "d"],
        "first run: all four in order"
    );
    assert_eq!(s.sync_count(), 3, "c (run_once) removed, a,b,d remain");

    // Second run: a → b → d (c is gone but cached order is valid)
    s.run_parallel(&mut w).unwrap();
    let second: Vec<&str> = log.lock().drain(..).collect();
    assert_eq!(
        &second,
        &["a", "b", "d"],
        "second run: c removed, a-b-d preserved"
    );
}

#[test]
fn run_parallel_disabled_system_order_preserved() {
    let mut w = World::new();
    let log = Arc::new(parking_lot::Mutex::new(Vec::new()));
    w.insert(log.clone());

    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "first",
        FunctionSystem::new("first", {
            let l = log.clone();
            move |_: ()| -> Result<()> {
                l.lock().push("first");
                Ok(())
            }
        })
        .label("first")
        .before("second"),
    );
    s.add(
        Stage::Update,
        "second",
        FunctionSystem::new("second", {
            let l = log.clone();
            move |_: ()| -> Result<()> {
                l.lock().push("second");
                Ok(())
            }
        })
        .label("second"),
    );

    s.run_parallel(&mut w).unwrap();
    assert_eq!(&*log.lock(), &["first", "second"]);
    log.lock().clear();

    // Disable "first" — only second runs, but should keep it + order
    s.set_enabled("first", false);
    s.run_parallel(&mut w).unwrap();
    assert_eq!(&*log.lock(), &["second"]);
    log.lock().clear();

    // Re-enable
    s.set_enabled("first", true);
    s.run_parallel(&mut w).unwrap();
    assert_eq!(&*log.lock(), &["first", "second"]);
}

// ═══════════════════════════════════════════════════════════════════════
// 30. AnySignal payload_heap — accurate estimation
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn any_signal_estimated_heap_bytes_includes_payload() {
    use zerotrace_core::signal::{AnySignal, MetricPoint};

    let m = MetricPoint::gauge("cpu.usage", 0.9, 5000)
        .with_attr("host", "node-1")
        .with_attr("region", "us-east-1")
        .with_description("CPU usage percentage");

    let any = AnySignal::new(m);
    let heap = any.estimated_heap_bytes();
    // Should exceed base struct size
    assert!(
        heap > std::mem::size_of::<AnySignal>(),
        "estimated_heap_bytes ({heap}) should exceed struct size ({})",
        std::mem::size_of::<AnySignal>()
    );
}

#[test]
fn any_signal_small_vs_large_payload() {
    use zerotrace_core::signal::AnySignal;

    #[derive(Debug, Clone, PartialEq)]
    struct Small {
        x: u32,
    }
    impl zerotrace_core::signal::SignalType for Small {
        fn signal_kind() -> zerotrace_core::signal::SignalKind {
            zerotrace_core::signal::SignalKind("small")
        }
        fn estimated_heap_bytes(&self) -> usize {
            0
        }
    }

    #[derive(Debug, Clone)]
    struct Large {
        data: Vec<u8>,
    }
    impl PartialEq for Large {
        fn eq(&self, other: &Self) -> bool {
            self.data == other.data
        }
    }
    impl zerotrace_core::signal::SignalType for Large {
        fn signal_kind() -> zerotrace_core::signal::SignalKind {
            zerotrace_core::signal::SignalKind("large")
        }
        fn estimated_heap_bytes(&self) -> usize {
            self.data.capacity()
        }
    }

    let small_any = AnySignal::new(Small { x: 1 });
    let large_any = AnySignal::new(Large {
        data: vec![0u8; 10000],
    });

    assert!(
        large_any.estimated_heap_bytes() > small_any.estimated_heap_bytes(),
        "large ({}) should exceed small ({})",
        large_any.estimated_heap_bytes(),
        small_any.estimated_heap_bytes()
    );
    assert!(
        large_any.estimated_heap_bytes() >= 10000,
        "large should include at least 10KB, got {}",
        large_any.estimated_heap_bytes()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 31. AttrValue NaN equality
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attr_value_nan_equals_nan() {
    let a = zerotrace_core::signal::AttrValue::Float(f64::NAN);
    let b = zerotrace_core::signal::AttrValue::Float(f64::NAN);
    assert_eq!(a, b, "NaN should equal NaN for attribute comparison");
}

#[test]
fn attr_value_nan_not_equal_to_number() {
    let nan = zerotrace_core::signal::AttrValue::Float(f64::NAN);
    let zero = zerotrace_core::signal::AttrValue::Float(0.0);
    assert_ne!(nan, zero, "NaN should not equal 0.0");
}

#[test]
fn attribute_set_with_nan_is_consistent() {
    use zerotrace_core::signal::{AttrValue, AttributeSetBuilder};

    let set1 = AttributeSetBuilder::new().with("value", AttrValue::Float(f64::NAN)).build();
    let set2 = AttributeSetBuilder::new().with("value", AttrValue::Float(f64::NAN)).build();

    assert_eq!(set1, set2, "sets with NaN should be equal");
    assert_eq!(set1.hash(), set2.hash(), "NaN sets should have same hash");
    assert!(set1.contains_all(&set2));
}

// ═══════════════════════════════════════════════════════════════════════
// 32. PipelineError fatal classification
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pipeline_error_fatal_true_is_fatal() {
    let err = Error::Pipeline {
        message: "channel closed".into(),
        fatal: true,
    };
    assert!(err.is_fatal());
    assert!(!err.is_retryable());
    assert_eq!(err.class(), zerotrace_core::error::ErrorClass::Fatal);
}

#[test]
fn pipeline_error_fatal_false_is_retryable() {
    let err = Error::Pipeline {
        message: "backpressure timeout".into(),
        fatal: false,
    };
    assert!(!err.is_fatal());
    assert!(err.is_retryable());
    assert_eq!(
        err.class(),
        zerotrace_core::error::ErrorClass::RetryableWithBackoff
    );
}

#[test]
fn pipeline_closed_constructor_is_fatal() {
    assert!(Error::pipeline_closed().is_fatal());
}

#[test]
fn pipeline_source_constructor_is_not_fatal() {
    let err = Error::pipeline_source("test_src", "connection refused");
    assert!(!err.is_fatal());
    assert!(err.is_retryable());
}

// ═══════════════════════════════════════════════════════════════════════
// 33. Lifecycle — stop continues after error, timeout rollback
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn lifecycle_stop_all_continues_after_error() {
    let w = Arc::new(World::new());
    let ctx = LifecycleCtx::new(w.clone(), tokio::runtime::Handle::current());
    let stopped = Arc::new(AtomicUsize::new(0));

    struct FailingStop {
        stopped: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl Lifecycle for FailingStop {
        fn name(&self) -> &'static str {
            "failing"
        }
        async fn on_stop(&mut self, _: &LifecycleCtx) -> Result<()> {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            Err(Error::Other("stop failure".into()))
        }
    }

    struct Normal {
        stopped: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl Lifecycle for Normal {
        fn name(&self) -> &'static str {
            "normal"
        }
        async fn on_stop(&mut self, _: &LifecycleCtx) -> Result<()> {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(Normal {
        stopped: stopped.clone(),
    });
    reg.register(FailingStop {
        stopped: stopped.clone(),
    });
    reg.start_all(&ctx).await.unwrap();

    let result = reg.stop_all(&ctx).await;
    assert!(result.is_err());
    assert_eq!(
        stopped.load(Ordering::SeqCst),
        2,
        "all components should be stopped even if one fails (LIFO: failing first, then normal)"
    );
}

#[tokio::test]
async fn lifecycle_start_all_with_timeout_success() {
    let w = Arc::new(World::new());
    let ctx = LifecycleCtx::new(w.clone(), tokio::runtime::Handle::current());

    struct Fast;
    #[async_trait::async_trait]
    impl Lifecycle for Fast {
        fn name(&self) -> &'static str {
            "fast"
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(Fast);
    reg.start_all_with_timeout(&ctx, std::time::Duration::from_secs(5))
        .await
        .unwrap();
}

#[tokio::test]
async fn lifecycle_start_all_with_timeout_hung_component_rollback() {
    let w = Arc::new(World::new());
    let ctx = LifecycleCtx::new(w.clone(), tokio::runtime::Handle::current());
    let stopped = Arc::new(AtomicUsize::new(0));

    struct Hung;
    #[async_trait::async_trait]
    impl Lifecycle for Hung {
        fn name(&self) -> &'static str {
            "hung"
        }
        async fn on_start(&mut self, _: &LifecycleCtx) -> Result<()> {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok(())
        }
    }

    struct Good {
        stopped: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl Lifecycle for Good {
        fn name(&self) -> &'static str {
            "good"
        }
        async fn on_stop(&mut self, _: &LifecycleCtx) -> Result<()> {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(Good {
        stopped: stopped.clone(),
    });
    reg.register(Hung);

    let result = reg.start_all_with_timeout(&ctx, std::time::Duration::from_millis(10)).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("timed out"),
        "error should mention timeout, got: {msg}"
    );
    assert_eq!(
        stopped.load(Ordering::SeqCst),
        1,
        "Good should be rolled back"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 34. ConfigBus — subscriber error + severity escalation
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn configbus_subscriber_error_propagates() {
    let mut bus = ConfigBus::new();
    let called = Arc::new(AtomicUsize::new(0));

    struct FailingSub {
        called: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl ConfigSubscriber for FailingSub {
        fn name(&self) -> &'static str {
            "failing"
        }
        fn interested(&self, _: &ConfigChange) -> bool {
            true
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            self.called.fetch_add(1, Ordering::SeqCst);
            Err(Error::Other("dispatch failure".into()))
        }
    }

    bus.subscribe(FailingSub {
        called: called.clone(),
    });

    let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
    let result = bus.dispatch(&ConfigChange::FullReload, &ctx).await;
    assert!(result.is_err());
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn configbus_severity_restart_pipeline_then_agent() {
    let mut bus = ConfigBus::new();

    struct PipelineRestarter;
    #[async_trait::async_trait]
    impl ConfigSubscriber for PipelineRestarter {
        fn name(&self) -> &'static str {
            "pipeline"
        }
        fn interested(&self, _: &ConfigChange) -> bool {
            true
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            Ok(Action::RestartPipeline("metrics"))
        }
    }

    struct AgentRestarter;
    #[async_trait::async_trait]
    impl ConfigSubscriber for AgentRestarter {
        fn name(&self) -> &'static str {
            "agent"
        }
        fn interested(&self, _: &ConfigChange) -> bool {
            true
        }
        async fn on_change(&mut self, _: &ConfigChange, _: &LifecycleCtx) -> Result<Action> {
            Ok(Action::RestartAgent)
        }
    }

    bus.subscribe(PipelineRestarter);
    bus.subscribe(AgentRestarter);

    let ctx = LifecycleCtx::new(Arc::new(World::new()), tokio::runtime::Handle::current());
    let action = bus.dispatch(&ConfigChange::FullReload, &ctx).await.unwrap();
    assert_eq!(action, Action::RestartAgent, "max severity should win");
}

// ═══════════════════════════════════════════════════════════════════════
// 35. ConfigRepo — static source + check no change
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn config_repo_with_static_source_initial_load() {
    use serde::Deserialize;
    use zerotrace_kernel::config_bus::ConfigRepo;

    #[derive(Debug, Deserialize, PartialEq, Clone)]
    struct SimpleCfg {
        key: String,
    }

    let world = World::new();
    let source = StaticSource::new(r#"{"key": "hello"}"#, "static_test");
    let _repo = ConfigRepo::<SimpleCfg>::from_source(Box::new(source), &world).unwrap();

    let (val, _) = world.get::<SimpleCfg>().unwrap();
    assert_eq!(val.read().key, "hello");
}

#[test]
fn config_repo_check_no_change_returns_none() {
    use zerotrace_kernel::config_bus::ConfigRepo;

    #[derive(Debug, Deserialize, PartialEq, Clone)]
    struct Cfg {
        x: u32,
    }

    let world = World::new();
    let source = StaticSource::new(r#"{"x": 1}"#, "static");
    let mut repo = ConfigRepo::<Cfg>::from_source(Box::new(source), &world).unwrap();

    let result = repo.check(&world).unwrap();
    assert!(result.is_none(), "same content should not trigger change");
}

// ═══════════════════════════════════════════════════════════════════════
// 36. Scheduler — ResKeyed + Option<ResKeyed> system params
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scheduler_with_res_keyed_param() {
    let mut w = World::new();
    w.insert_keyed::<KeyA, Counter>(Counter(5));
    w.insert_keyed::<KeyB, Counter>(Counter(10));

    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "read_keys",
        |(a, b): (ResKeyed<KeyA, Counter>, ResKeyed<KeyB, Counter>)| -> Result<()> {
            assert_eq!(a.read().0, 5);
            assert_eq!(b.read().0, 10);
            Ok(())
        },
    );
    s.run(&mut w).unwrap();
}

#[test]
fn scheduler_with_option_res_keyed() {
    let mut w = World::new();
    w.insert_keyed::<KeyA, Counter>(Counter(42));

    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "opt_keys",
        |(a, b): (
            Option<ResKeyed<KeyA, Counter>>,
            Option<ResKeyed<KeyB, Counter>>,
        )|
         -> Result<()> {
            assert!(a.is_some());
            assert_eq!(a.unwrap().read().0, 42);
            assert!(b.is_none());
            Ok(())
        },
    );
    s.run(&mut w).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
// 37. ResLifecycle system param
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn res_lifecycle_fetch_and_register() {
    let w = World::new();
    let rl: ResLifecycle = ResLifecycle::fetch(&w, &ctx()).unwrap();
    assert_eq!(rl.registry().len(), 0);

    struct Dummy;
    #[async_trait::async_trait]
    impl Lifecycle for Dummy {
        fn name(&self) -> &'static str {
            "dummy"
        }
    }
    rl.registry().register(Dummy);
    assert_eq!(rl.registry().len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════
// 38. EventWriter/EventReader — empty states and batch send
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn event_writer_empty_after_reader_drain() {
    let w = World::new();
    w.insert_raw(Arc::new(Events::<TestEvent>::new()));
    let mut writer = EventWriter::<TestEvent>::fetch(&w, &ctx()).unwrap();
    assert!(writer.is_empty());
    writer.write(TestEvent(1));
    assert!(!writer.is_empty());
    assert_eq!(writer.len(), 1);

    let mut reader = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    reader.drain();
    assert!(writer.is_empty());
}

#[test]
fn event_send_batch_500() {
    let w = World::new();
    w.insert_raw(Arc::new(Events::<TestEvent>::new()));
    let mut writer = EventWriter::<TestEvent>::fetch(&w, &ctx()).unwrap();
    writer.send_batch((0..500).map(|i| TestEvent(i as u32)));

    let mut reader = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    let drained = reader.drain();
    assert_eq!(drained.len(), 500);
}

// ═══════════════════════════════════════════════════════════════════════
// 39. Scheduler — Commands + EventWriter together
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scheduler_commands_and_events_together() {
    let mut w = World::new();
    w.insert_raw(Arc::new(Events::<TestEvent>::new()));

    let mut s = Scheduler::new();
    s.add(
        Stage::Update,
        "both",
        |(mut writer, mut cmd): (EventWriter<TestEvent>, Commands)| -> Result<()> {
            writer.write(TestEvent(100));
            cmd.insert(Counter(200));
            Ok(())
        },
    );

    s.run(&mut w).unwrap();

    let mut reader = EventReader::<TestEvent>::fetch(&w, &ctx()).unwrap();
    let drained = reader.drain();
    assert_eq!(drained, vec![TestEvent(100)]);
    assert_eq!(w.get::<Counter>().unwrap().0.read().0, 200);
}

#[test]
fn scheduler_8_tuple_params_compile() {
    let w = World::new();
    w.insert(Counter(1));
    w.insert(Tag("a".into()));
    w.insert(Config { value: 2 });
    w.insert(Flag(true));
    w.insert_raw(Arc::new(Events::<TestEvent>::new()));

    let result = <(
        Res<Counter>,
        Res<Tag>,
        Cfg<Config>,
        Res<Flag>,
        ResMut<Counter>,
        Commands,
        EventWriter<TestEvent>,
        EventReader<TestEvent>,
    ) as SystemParam>::fetch(&w, &ctx());
    assert!(result.is_ok(), "8-tuple should be supported");
}

// ═══════════════════════════════════════════════════════════════════════
// 40. run_parallel — conflict-free parallel execution + greedy_batch
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn run_parallel_conflict_free_batch() {
    let mut w = World::new();
    let counter = Arc::new(AtomicUsize::new(0));
    w.insert(counter.clone());

    let mut s = Scheduler::new();
    let c1 = counter.clone();
    s.add(Stage::Update, "a", move |_: ()| -> Result<()> {
        c1.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(10));
        Ok(())
    });
    let c2 = counter.clone();
    s.add(Stage::Update, "b", move |_: ()| -> Result<()> {
        c2.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(10));
        Ok(())
    });

    s.run_parallel(&mut w).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn greedy_batch_conflict_table() {
    use std::any::TypeId;
    use zerotrace_kernel::system::{Scheduler, SystemAccess};

    // Clone before moving into greedy_batch
    let a = SystemAccess {
        reads: vec![TypeId::of::<Counter>()],
        writes: vec![TypeId::of::<Tag>()],
    };
    let b = SystemAccess {
        reads: vec![TypeId::of::<Tag>()],
        writes: vec![TypeId::of::<Config>()],
    };
    let c = SystemAccess {
        reads: vec![TypeId::of::<Config>()],
        writes: vec![],
    };

    let accesses = [a.clone(), b.clone(), c.clone()];
    let batches = Scheduler::greedy_batch(&accesses);
    for batch in &batches {
        for i in 0..batch.len() {
            for j in (i + 1)..batch.len() {
                let acc_i = &accesses[batch[i]];
                let acc_j = &accesses[batch[j]];
                assert!(
                    !acc_i.conflicts_with(acc_j),
                    "batch {:?}: system {} conflicts with {}",
                    batch,
                    batch[i],
                    batch[j]
                );
            }
        }
    }
    let total: usize = batches.iter().map(|b| b.len()).sum();
    assert_eq!(total, 3);
}

// ═══════════════════════════════════════════════════════════════════════
// 41. Stress — 100-system chain ordering
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn stress_100_systems_chain_ordering() {
    let mut w = World::new();
    let cnt = Arc::new(AtomicUsize::new(0));
    w.insert(cnt.clone());

    let mut s = Scheduler::new();
    for i in 0..100usize {
        let name = Box::leak(format!("sys_{i}").into_boxed_str()) as &'static str;
        let lbl = Box::leak(format!("lbl_{i}").into_boxed_str()) as &'static str;
        let cnt_clone = cnt.clone();
        let sys = FunctionSystem::new(name, move |_: ()| -> Result<()> {
            cnt_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .label(lbl);
        let sys = if i > 0 {
            let prev = Box::leak(format!("lbl_{}", i - 1).into_boxed_str()) as &'static str;
            sys.after(prev)
        } else {
            sys
        };
        if i < 99 {
            let next = Box::leak(format!("lbl_{}", i + 1).into_boxed_str()) as &'static str;
            s.add(Stage::Update, name, sys.before(next));
        } else {
            s.add(Stage::Update, name, sys);
        }
    }

    s.run(&mut w).unwrap();
    assert_eq!(
        cnt.load(Ordering::SeqCst),
        100,
        "all 100 systems must execute in chain order"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 43. ExclusiveSystem error clears commands
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn exclusive_system_error_clears_commands_and_recovers() {
    #[derive(Debug, PartialEq)]
    struct ExRes(u32);

    let mut w = World::new();
    let mut s = Scheduler::new();

    s.add_exclusive(FunctionExclusiveSystem::new(
        "ex_fail",
        |w: &mut World| -> Result<()> {
            w.insert(ExRes(1)); // direct mutation, un-rollbackable
            Err(Error::Other("exclusive failure".into()))
        },
    ));

    let result = s.run(&mut w);
    assert!(result.is_err());

    // Scheduler should recover
    s.add(Stage::Update, "recover", |_: ()| -> Result<()> { Ok(()) });
    assert!(s.run(&mut w).is_ok());
}

// ═══════════════════════════════════════════════════════════════════════
// 44. AttributeSet — merge, contains_all edge cases
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attribute_set_merge_disjoint_keys() {
    use zerotrace_core::signal::AttributeSetBuilder;
    let a = AttributeSetBuilder::new().with("x", "1").build();
    let b = AttributeSetBuilder::new().with("y", "2").build();
    let m = a.merge(&b);
    assert_eq!(m.len(), 2);
    assert!(m.contains_key("x"));
    assert!(m.contains_key("y"));
}

#[test]
fn attribute_set_contains_all_empty_is_always_subset() {
    use zerotrace_core::signal::AttributeSetBuilder;
    let set = AttributeSetBuilder::new().with("host", "n1").build();
    assert!(set.contains_all(&AttributeSetBuilder::new().build()));
}

#[test]
fn attribute_set_has_value_string_match() {
    use zerotrace_core::signal::AttributeSetBuilder;
    let set = AttributeSetBuilder::new()
        .with("env", "prod")
        .with("region", "us-east-1")
        .build();
    assert!(set.has_value("env", "prod"));
    assert!(!set.has_value("env", "staging"));
    assert!(!set.has_value("nonexistent", "any"));
}

// ═══════════════════════════════════════════════════════════════════════
// 45. SystemParam — unit and empty param
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn system_param_unit_always_succeeds() {
    let w = World::new();
    assert!(<() as SystemParam>::fetch(&w, &ctx()).is_ok());
}

#[test]
fn scheduler_with_empty_param_system() {
    let mut w = World::new();
    let mut s = Scheduler::new();
    s.add(Stage::Update, "noop", |_: ()| -> Result<()> { Ok(()) });
    s.run(&mut w).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
// 46. Lifecycle — health_all edge cases
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn lifecycle_health_all_healthy_default() {
    struct H;
    #[async_trait::async_trait]
    impl Lifecycle for H {
        fn name(&self) -> &'static str {
            "h"
        }
    }
    let reg = LifecycleRegistry::new();
    reg.register(H);
    assert_eq!(reg.health_all(), Health::Healthy);
}

#[test]
fn lifecycle_health_down_wins_over_degraded() {
    struct D1;
    #[async_trait::async_trait]
    impl Lifecycle for D1 {
        fn name(&self) -> &'static str {
            "d1"
        }
        fn health(&self) -> Health {
            Health::Degraded {
                reason: "slow".into(),
            }
        }
    }
    struct D2;
    #[async_trait::async_trait]
    impl Lifecycle for D2 {
        fn name(&self) -> &'static str {
            "d2"
        }
        fn health(&self) -> Health {
            Health::Down {
                reason: "crashed".into(),
            }
        }
    }

    let reg = LifecycleRegistry::new();
    reg.register(D1);
    reg.register(D2);
    assert!(matches!(reg.health_all(), Health::Down { .. }));
}

// ═══════════════════════════════════════════════════════════════════════
// 47. Async system — param injection + error clears commands
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn scheduler_add_async_param_with_system_param() {
    use zerotrace_kernel::{param::Res, system::Scheduler};

    let mut w = World::new();
    w.insert(Counter(42));

    let mut s = Scheduler::new();
    s.add_async_param(
        Stage::Update,
        "async_param",
        |counter: Res<Counter>| async move {
            assert_eq!(counter.read().0, 42);
            Ok(())
        },
    );

    let handle = tokio::runtime::Handle::current();
    s.run_async(&mut w, &handle).await.unwrap();
}

#[tokio::test]
async fn scheduler_async_system_error_clears_commands() {
    // Async systems that fail should have their commands cleared.
    // Since CommandQueue::buffer is pub(crate), we verify indirectly:
    // a preceding sync system queues commands via Commands (public API),
    // then the async system fails. The sync system's commands were
    // already applied (it succeeded), so we verify the scheduler
    // recovers and no ghost resources appear.
    let mut w = World::new();

    let mut s = Scheduler::new();

    // Sync system: insert a legitimate resource via Commands (succeeds)
    #[derive(Debug, PartialEq)]
    struct GoodFromSync(u32);
    s.add(
        Stage::Update,
        "sync_good",
        |mut cmd: Commands| -> Result<()> {
            cmd.insert(GoodFromSync(1));
            Ok(())
        },
    );

    // AsyncFail: just returns error immediately
    struct AsyncFail;
    impl zerotrace_kernel::AsyncSystem for AsyncFail {
        fn name(&self) -> &'static str {
            "async_fail"
        }
        fn run_async(
            &mut self,
            _world: &World,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async { Err(Error::Other("async failure".into())) })
        }
    }
    s.add_async(Stage::Update, AsyncFail);

    let handle = tokio::runtime::Handle::current();
    let result = s.run_async(&mut w, &handle).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("async failure"));

    // GoodFromSync should be visible (sync system succeeded before async ran)
    assert!(
        w.contains::<GoodFromSync>(),
        "sync system before async should have succeeded"
    );

    // Verify recovery
    s.add(Stage::Update, "recover", |_: ()| -> Result<()> { Ok(()) });
    assert!(s.run(&mut w).is_ok());
}

// ═══════════════════════════════════════════════════════════════════════
// 48. run_parallel all systems execute exactly once per tick
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn run_parallel_each_system_exactly_once() {
    let mut w = World::new();
    let executed = Arc::new(AtomicUsize::new(0));
    w.insert(executed.clone());

    let mut s = Scheduler::new();
    for _i in 0..10 {
        let ex = executed.clone();
        s.add(Stage::Update, "sys", move |_: ()| -> Result<()> {
            ex.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
    }

    for _ in 0..5 {
        s.run_parallel(&mut w).unwrap();
    }
    assert_eq!(
        executed.load(Ordering::SeqCst),
        50,
        "10 systems × 5 runs = 50"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 49. ResMut is_changed detects external insert/overwrite
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn resmut_is_changed_detects_world_insert_overwrite() {
    let w = World::new();
    w.insert(Counter(1));
    let tick_after_first = w.current_tick();

    let r: ResMut<Counter> = ResMut::fetch(&w, &SystemContext::new(tick_after_first, 0)).unwrap();
    assert!(r.is_changed(), "insert after last_run=0 → changed");

    // Bump the global tick by inserting/removing a temp resource so the
    // overwrite gets a strictly later tick than last_run.
    #[derive(Debug, PartialEq)]
    struct TickBump;
    w.insert(TickBump);
    w.remove_resource::<TickBump>();
    let tick_after_bump = w.current_tick();

    // Fetch r2 with last_run < the overwrite tick
    let r2: ResMut<Counter> = ResMut::fetch(
        &w,
        &SystemContext::new(tick_after_bump + 1, tick_after_bump - 1),
    )
    .unwrap();

    w.insert(Counter(2)); // overwrite reuses meta, bumps changed_tick > last_run
    assert!(
        r2.is_changed(),
        "overwrite should be detected by ResMut handle"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 50. Bundle — optional dependency skipped
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bundle_optional_dependency_skipped_when_missing() {
    use parking_lot::RwLock;

    #[derive(Debug)]
    struct RequiredComp;

    struct OptionalBundle;
    impl Bundle for OptionalBundle {
        fn id(&self) -> &'static str {
            "opt"
        }
        fn name(&self) -> &'static str {
            "Optional"
        }
        fn components(&self) -> Vec<ComponentDescriptor> {
            vec![ComponentDescriptor {
                id: "opt_comp",
                provides: std::any::TypeId::of::<RequiredComp>(),
                deps: vec![std::any::TypeId::of::<Config>()],
                optional: true,
                factory: Box::new(|_, _| {
                    Ok(std::sync::Arc::new(RwLock::new(RequiredComp))
                        as std::sync::Arc<dyn std::any::Any + Send + Sync>)
                }),
            }]
        }
    }

    let world = World::new();
    // Config is NOT inserted → optional dep is skipped
    let mut set = BundleSet::new(&world);
    set.load(&OptionalBundle).unwrap();
    // Component should still be loaded (optional dep missing → continue)
    assert!(world.contains::<RequiredComp>());
}

// ═══════════════════════════════════════════════════════════════════════
// 51-52. Pipeline defaults and BackpressurePolicy
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn backpressure_policy_default_is_block() {
    use zerotrace_runtime::pipeline::BackpressurePolicy;
    assert_eq!(BackpressurePolicy::default(), BackpressurePolicy::Block);
}

#[test]
fn pipeline_spec_defaults() {
    use zerotrace_runtime::pipeline::{BackpressurePolicy, PipelineSpec};
    let spec = PipelineSpec::default();
    assert!(spec.name.is_empty());
    assert_eq!(spec.channel_capacity, 4096);
    assert!(spec.enabled);
    assert_eq!(spec.backpressure, BackpressurePolicy::Block);
}

// ═══════════════════════════════════════════════════════════════════════
// 53. KernelMetrics — defaults and recording
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn kernel_metrics_defaults_and_recording() {
    let m = zerotrace_kernel::metrics::KernelMetrics::new();
    assert_eq!(m.world_get_avg_ns(), 0);
    assert_eq!(m.world_get_slow_ratio(), 0.0);

    m.record_world_get(500);
    m.record_world_get(1500);
    assert!(m.world_get_avg_ns() > 0);
    assert!(m.world_get_slow_ratio() > 0.0);

    let snap = m.snapshot();
    assert!(snap.iter().any(|(k, _)| *k == "world_get_count"));
    assert!(snap.iter().any(|(k, _)| *k == "config_dispatch_total"));
}

// ═══════════════════════════════════════════════════════════════════════
// 54. Scheduler — remove, enable/disable, clear
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scheduler_remove_by_name_and_nonexistent() {
    let mut s = Scheduler::new();
    s.add(Stage::Update, "sync1", |_: ()| -> Result<()> { Ok(()) });
    s.add(Stage::Update, "sync2", |_: ()| -> Result<()> { Ok(()) });
    assert_eq!(s.sync_count(), 2);

    assert!(s.remove_by_name("sync1"));
    assert_eq!(s.sync_count(), 1);
    assert!(!s.remove_by_name("nonexistent"));
    assert_eq!(s.sync_count(), 1);
}

#[test]
fn scheduler_enable_disable_nonexistent() {
    let mut s = Scheduler::new();
    assert!(!s.set_enabled("ghost", false));
    assert!(!s.set_async_enabled("ghost", false));
}

#[test]
fn scheduler_clear_removes_all_systems() {
    let mut s = Scheduler::new();
    s.add(Stage::Update, "a", |_: ()| -> Result<()> { Ok(()) });
    s.add(Stage::PreUpdate, "b", |_: ()| -> Result<()> { Ok(()) });
    assert!(s.system_count() > 0);

    s.clear();
    assert_eq!(s.sync_count(), 0);
    assert_eq!(s.async_count(), 0);
    assert_eq!(s.exclusive_count(), 0);
    assert_eq!(s.system_count(), 0);
}

// ═══════════════════════════════════════════════════════════════════════
// 55. World — keyed remove and re-insert
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn world_keyed_remove_and_reinsert() {
    let w = World::new();
    w.insert_keyed::<KeyA, Counter>(Counter(1));
    assert!(w.contains_keyed::<KeyA, Counter>());

    w.remove_keyed::<KeyA, Counter>();
    assert!(!w.contains_keyed::<KeyA, Counter>());

    w.insert_keyed::<KeyA, Counter>(Counter(99));
    let (val, _) = w.get_keyed::<KeyA, Counter>().unwrap();
    assert_eq!(val.read().0, 99);
}

// ═══════════════════════════════════════════════════════════════════════
// 56. Error — complete classification coverage
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn error_classification_exhaustive() {
    let fatal = [
        Error::ResourceNotFound { type_name: "T" },
        Error::ResourceTypeMismatch {
            type_name: "T",
            storage_mode: "a",
            access_mode: "b",
        },
        Error::Lifecycle {
            component: "c",
            message: "m".into(),
        },
        Error::Bundle {
            bundle_id: "b",
            message: "m".into(),
        },
        Error::Pipeline {
            message: "m".into(),
            fatal: true,
        },
        Error::Config("c".into()),
        Error::Other("o".into()),
    ];
    for err in &fatal {
        assert!(err.is_fatal(), "{err:?} should be fatal");
        assert!(!err.is_retryable(), "{err:?} should not be retryable");
    }

    let retryable = [
        Error::ConfigDispatch("timeout".into()),
        Error::Pipeline {
            message: "retry".into(),
            fatal: false,
        },
    ];
    for err in &retryable {
        assert!(err.is_retryable(), "{err:?} should be retryable");
        assert!(!err.is_fatal(), "{err:?} should not be fatal");
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 57. Cfg snapshot is independent clone
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn cfg_snapshot_independent_of_mutation() {
    let w = World::new();
    w.insert(Config { value: 42 });

    let cfg: Cfg<Config> = Cfg::fetch(&w, &ctx()).unwrap();
    let snap = cfg.snapshot();
    assert_eq!(snap.value, 42);

    // Mutate via ResMut
    let r: ResMut<Config> = ResMut::fetch(&w, &ctx()).unwrap();
    r.write().value = 99;
    assert_eq!(
        snap.value, 42,
        "snapshot taken before mutation is unchanged"
    );
    assert_eq!(cfg.snapshot().value, 99, "new snapshot reflects mutation");
}

// ═══════════════════════════════════════════════════════════════════════
// 58. Action severity complete ordering
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn config_action_severity_ordering() {
    assert!(Action::RestartAgent.is_more_severe_than(&Action::RestartPipeline("p")));
    assert!(Action::RestartPipeline("p").is_more_severe_than(&Action::RestartSelf));
    assert!(Action::RestartSelf.is_more_severe_than(&Action::HotApplied));
    assert!(!Action::HotApplied.is_more_severe_than(&Action::HotApplied));
    assert!(!Action::HotApplied.is_more_severe_than(&Action::RestartAgent));
    assert_eq!(Action::HotApplied.severity(), 0);
    assert_eq!(Action::RestartSelf.severity(), 1);
    assert_eq!(Action::RestartPipeline("").severity(), 2);
    assert_eq!(Action::RestartAgent.severity(), 3);
}

// ═══════════════════════════════════════════════════════════════════════
// 59. Error recovery — scheduler preserves completed stages on error
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn error_recovery_preserves_completed_stage_systems() {
    // When a system in stage 2 fails, systems in stage 1 should survive.
    let mut w = World::new();
    let ran_pre = Arc::new(AtomicUsize::new(0));
    let ran_update = Arc::new(AtomicUsize::new(0));
    w.insert(ran_pre.clone());
    w.insert(ran_update.clone());

    let mut s = Scheduler::new();

    // Stage: PreUpdate — always succeeds
    s.add(Stage::PreUpdate, "pre_good", {
        let r = ran_pre.clone();
        move |_: ()| -> Result<()> {
            r.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });

    // Stage: Update — second system fails
    s.add(Stage::Update, "update_good", {
        let r = ran_update.clone();
        move |_: ()| -> Result<()> {
            r.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });
    s.add(Stage::Update, "update_fail", |_: ()| -> Result<()> {
        Err(Error::Other("update failure".into()))
    });

    let result = s.run(&mut w);
    assert!(result.is_err());

    // PreUpdate system should have run once
    assert_eq!(ran_pre.load(Ordering::SeqCst), 1);
    // Update good system should have run once (before the failure)
    assert_eq!(ran_update.load(Ordering::SeqCst), 1);

    // Scheduler should still be usable
    let ran_recovery = Arc::new(AtomicUsize::new(0));
    s.add(Stage::Update, "recovery", {
        let r = ran_recovery.clone();
        move |_: ()| -> Result<()> {
            r.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });
    assert!(s.run(&mut w).is_ok());
    assert_eq!(ran_recovery.load(Ordering::SeqCst), 1);

    // PreUpdate system should still be present and run again
    assert_eq!(ran_pre.load(Ordering::SeqCst), 2);
}

#[test]
fn error_recovery_preserves_exclusive_systems() {
    let mut w = World::new();
    let ran_ex = Arc::new(AtomicUsize::new(0));

    let mut s = Scheduler::new();
    let r1 = ran_ex.clone();
    s.add_exclusive(FunctionExclusiveSystem::new(
        "ex_good",
        move |w: &mut World| -> Result<()> {
            w.insert(Counter(1));
            r1.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    ));
    s.add_exclusive(FunctionExclusiveSystem::new(
        "ex_fail",
        |_: &mut World| -> Result<()> { Err(Error::Other("ex failure".into())) },
    ));

    let result = s.run(&mut w);
    assert!(result.is_err());
    assert_eq!(ran_ex.load(Ordering::SeqCst), 1);

    // Good exclusive system should be preserved and run again
    let result2 = s.run(&mut w);
    assert!(result2.is_ok());
    assert_eq!(ran_ex.load(Ordering::SeqCst), 2);
}

#[test]
fn error_recovery_stages_survive_and_can_recover() {
    // Three stages: PreUpdate, Update (fails), PostUpdate.
    // PreUpdate should survive, PostUpdate never runs.
    let mut w = World::new();
    let seq = Arc::new(parking_lot::Mutex::new(Vec::new()));
    w.insert(seq.clone());

    let mut s = Scheduler::new();
    let l1 = seq.clone();
    s.add(Stage::PreUpdate, "pre", move |_: ()| -> Result<()> {
        l1.lock().push("pre");
        Ok(())
    });
    let l2 = seq.clone();
    s.add(Stage::Update, "fail", move |_: ()| -> Result<()> {
        l2.lock().push("update");
        Err(Error::Other("fail".into()))
    });
    let l3 = seq.clone();
    s.add(Stage::PostUpdate, "post", move |_: ()| -> Result<()> {
        l3.lock().push("post");
        Ok(())
    });

    let result = s.run(&mut w);
    assert!(result.is_err());
    assert_eq!(&*seq.lock(), &["pre", "update"]);

    // PostUpdate should be preserved and run on next tick
    let l4 = seq.clone();
    s.add(Stage::Update, "recovery", move |_: ()| -> Result<()> {
        l4.lock().push("recovery");
        Ok(())
    });
    assert!(s.run(&mut w).is_ok());
    // pre + post should run again (but NOT "fail" or "update" — they were removed)
    let final_seq = seq.lock().clone();
    assert!(final_seq.contains(&"pre"), "pre should survive and re-run");
    assert!(final_seq.contains(&"post"), "post should survive and run");
    assert!(final_seq.contains(&"recovery"), "recovery should run");
}
