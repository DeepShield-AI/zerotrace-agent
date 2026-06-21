// Benchmarks for the zerotrace-kernel DI framework.
// Run with: cargo bench --package zerotrace-kernel

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::sync::Arc;
use zerotrace_kernel::{
    event::Events,
    param::{EventReader, EventWriter, SystemParam},
    world::{SystemContext, World},
};

fn ctx() -> SystemContext {
    SystemContext::new(2, 1)
}

fn bench_world_insert_get(c: &mut Criterion) {
    #[derive(Debug)]
    struct Value(u64);

    c.bench_function("world_insert_and_get", |b| {
        b.iter(|| {
            let w = World::new();
            for i in 0..100 {
                w.insert(Value(i));
                let (val, _) = w.get::<Value>().unwrap();
                black_box(val.read().0);
            }
        })
    });
}

fn bench_events_throughput(c: &mut Criterion) {
    #[derive(Debug, Clone, PartialEq)]
    struct Ev(u64);

    c.bench_function("events_send_10k", |b| {
        b.iter(|| {
            let w = World::new();
            w.insert_raw(Arc::new(Events::<Ev>::new()));
            let mut writer = EventWriter::<Ev>::fetch(&w, &ctx()).unwrap();
            for i in 0..10_000 {
                writer.write(Ev(i));
            }
            black_box(writer.len());
        })
    });

    c.bench_function("events_drain_10k", |b| {
        let w = World::new();
        w.insert_raw(Arc::new(Events::<Ev>::new()));
        let mut writer = EventWriter::<Ev>::fetch(&w, &ctx()).unwrap();
        for i in 0..10_000 {
            writer.write(Ev(i));
        }
        b.iter(|| {
            let mut reader = EventReader::<Ev>::fetch(&w, &ctx()).unwrap();
            black_box(reader.drain().len());
        })
    });
}

fn bench_scheduler_100_systems(c: &mut Criterion) {
    use zerotrace_kernel::{
        error::Result,
        system::{Scheduler, Stage},
    };

    #[derive(Debug)]
    struct Counter(u64);

    c.bench_function("scheduler_run_100_systems", |b| {
        b.iter(|| {
            let mut w = World::new();
            w.insert(Counter(0));
            let mut s = Scheduler::new();
            for _ in 0..100 {
                s.add(Stage::Update, "inc", |_: ()| -> Result<()> { Ok(()) });
            }
            s.run(&mut w).unwrap();
            black_box(w.get::<Counter>());
        })
    });
}

fn bench_scheduler_with_ordering(c: &mut Criterion) {
    use zerotrace_kernel::{
        error::Result,
        system::{FunctionSystem, Scheduler, Stage},
    };

    #[derive(Debug)]
    struct Counter(u64);

    // Intentionally leaked to create `&'static str` labels for the
    // benchmark.  The leak is bounded (50 strings) and only lives for
    // the duration of the benchmark process.
    let labels: Vec<&'static str> = (0..50)
        .map(|i| {
            let s: String = format!("lbl_{}", i);
            // SAFETY: leaking a String to obtain a &'static str is
            // safe and intentional — the memory is reclaimed when the
            // benchmark process exits.
            Box::leak(s.into_boxed_str()) as &'static str
        })
        .collect();

    c.bench_function("scheduler_run_50_ordered_systems", |b| {
        b.iter(|| {
            let mut w = World::new();
            w.insert(Counter(0));
            let mut s = Scheduler::new();
            for i in 0..50 {
                let sys =
                    FunctionSystem::new("sys", |_: ()| -> Result<()> { Ok(()) }).label(labels[i]);
                if i + 1 < 50 {
                    s.add(Stage::Update, "sys", sys.before(labels[i + 1]));
                } else {
                    s.add(Stage::Update, "sys", sys);
                }
            }
            s.run(&mut w).unwrap();
            black_box(w.get::<Counter>());
        })
    });
}

criterion_group!(
    benches,
    bench_world_insert_get,
    bench_events_throughput,
    bench_scheduler_100_systems,
    bench_scheduler_with_ordering,
);
criterion_main!(benches);
