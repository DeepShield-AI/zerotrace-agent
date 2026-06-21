//! ZeroTrace DI Framework Demo — 入口。
//!
//! # 两种配置模式
//!
//! **简单模式** (`config/demo.yaml`):
//!   YAML `type:` 字段 → main.rs 用 `match type` 创建具体类型
//!   → 适合单 pipeline、类型固定的场景
//!
//! **ref: DSL 模式** (`config/demo_pipelines.yaml`):
//!   YAML `ref:` 字段 → 引用已注册的组件名
//!   → Bundle 只管注册组件，YAML 只管接线
//!   → 加新 pipeline 不改 main.rs 的一行 match 代码
//!
//! # 运行
//!
//! ```bash
//! cargo run -p zerotrace-demo                           # 简单模式
//! cargo run -p zerotrace-demo -- config/demo_pipelines.yaml  # ref: DSL 模式
//! ```

use parking_lot::RwLock;
use std::sync::Arc;
use zerotrace_demo::{
    bundles::{self, AgentSession, CollectorHandle, ForwarderHandle},
    collectors::{MockDemoSource, PeriodicDemoSource},
    config::DemoConfig,
    processors::{EnrichProcessor, NoopProcessor, ThresholdFilter},
    reporters::{ConsoleFormat, ConsoleReporter},
    signals::DemoMetric,
};
use zerotrace_kernel::{app::App, bundle::BundleSet, param::Cfg, system::Stage};
use zerotrace_runtime::{
    blueprint::PipelineBlueprint,
    pipeline::{BackpressurePolicy, PipelineHandle, PipelineSpec},
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    tracing::info!("🚀 ZeroTrace DI Framework Demo");

    // ── 1. 加载 YAML ──────────────────────────────────────────────────
    let config_path = std::env::args()
        .nth(1)
        .or_else(|| {
            std::env::var("CARGO_MANIFEST_DIR")
                .ok()
                .map(|d| format!("{d}/config/demo.yaml"))
        })
        .unwrap_or_else(|| "config/demo.yaml".into());

    let demo_config = DemoConfig::from_file(&config_path).unwrap_or_else(|e| {
        tracing::warn!("failed to load [{config_path}]: {e} — using defaults");
        DemoConfig::default()
    });

    let is_dsl_mode = demo_config.has_pipelines();
    tracing::info!(
        "📋 version={}, mode={}, bundles={:?}",
        demo_config.version,
        if is_dsl_mode { "ref: DSL" } else { "simple" },
        demo_config.bundles,
    );

    // ── 2. Bundle 加载（两种模式相同） ────────────────────────────────
    let mut app = App::new();
    let config = Arc::new(RwLock::new(demo_config.clone()));
    let session = Arc::new(RwLock::new(AgentSession {
        server_url: "https://server:3000".into(),
        connected: true,
    }));

    let mut loaded_bundles: Vec<String> = vec![];
    for bundle_name in &demo_config.bundles {
        match bundle_name.as_str() {
            "demo_core" => {
                let mut set = BundleSet::new(&app.world);
                set.load(&bundles::ConfigBundle {
                    config: config.clone(),
                })
                .unwrap();
                set.load(&bundles::SessionBundle {
                    session: session.clone(),
                })
                .unwrap();
                set.load(&bundles::ForwarderBundle {
                    forwarder: Arc::new(RwLock::new(ForwarderHandle {
                        session: session.clone(),
                        endpoint: "/api/v1/data/ingest".into(),
                    })),
                })
                .unwrap();
                set.load(&bundles::CollectorBundle {
                    collector: Arc::new(RwLock::new(CollectorHandle {
                        session: session.clone(),
                        config: config.clone(),
                        running: true,
                    })),
                })
                .unwrap();
                loaded_bundles.push(bundle_name.clone());
            },
            "demo_pipeline" => {
                loaded_bundles.push(bundle_name.clone());
            },
            other => tracing::warn!("unknown bundle [{other}] — skipping"),
        }
    }
    tracing::info!("✅ Bundles loaded: {:?}", loaded_bundles);

    // ── 3. 注册组件到 Blueprint ────────────────────────────────────────
    //
    //    关键区别：
    //    简单模式：从 YAML params 创建具体类型 → match type → 注册
    //    ref: DSL：预注册所有可用组件 → YAML ref: 选择哪些被接线
    // ────────────────────────────────────────────────────────────────────

    let mut bp = PipelineBlueprint::new();
    let mut signal_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));

    if is_dsl_mode {
        // ══════════════════════════════════════════════════════════════
        // ref: DSL 模式 — 预注册所有组件，YAML 只引用名字
        // ══════════════════════════════════════════════════════════════

        // Sources
        bp.add_source(
            "periodic_demo",
            PeriodicDemoSource::new("periodic_demo")
                .with_interval(500)
                .with_batch_size(5)
                .with_metric_names(vec![
                    "cpu.utilization".into(),
                    "memory.usage_bytes".into(),
                    "disk.io_ops".into(),
                ]),
        );

        // 模拟 L7 collector（用 Mock 替代）
        let mut mock_batch = zerotrace_core::signal::Batch::new(Arc::new(
            zerotrace_core::signal::BatchMetadata::new("mock_l7"),
        ));
        mock_batch.push(DemoMetric::gauge("l7.request_count", 42.0, 0));
        bp.add_source(
            "mock_l7_source",
            MockDemoSource::new("mock_l7").with_batches(vec![mock_batch]),
        );

        // Processors
        bp.add_processor(
            "enrich_env",
            EnrichProcessor::new("enrich", "environment", "demo"),
        );
        bp.add_processor(
            "enrich_collector",
            EnrichProcessor::new("enrich", "collector.name", "zerotrace-demo"),
        );
        bp.add_processor(
            "threshold_filter",
            ThresholdFilter::new("threshold_filter", 0.0),
        );
        bp.add_processor("l7_noop", NoopProcessor::new("l7_noop"));

        // Reporters — 两条 pipeline 需要两个独立实例（consuming 语义）
        let rep1 = ConsoleReporter::new("console_metrics")
            .with_format(ConsoleFormat::Summary)
            .with_log_interval(5);
        signal_counter = rep1.signal_count();
        bp.add_reporter("console", rep1);

        let rep2 = ConsoleReporter::new("console_l7")
            .with_format(ConsoleFormat::Summary)
            .with_log_interval(100);
        bp.add_reporter("console_l7", rep2);

        tracing::info!(
            "🔌 Components pre-registered: {} sources, {} processors, {} reporters",
            bp.source_count(),
            bp.processor_count(),
            bp.reporter_count()
        );
    } else {
        // ══════════════════════════════════════════════════════════════
        // 简单模式 — 从 YAML type: 字段创建组件
        // ══════════════════════════════════════════════════════════════

        let src_cfg = demo_config.source.as_ref().expect("source required in simple mode");
        match src_cfg.source_type.as_str() {
            "periodic_demo" => {
                let s = PeriodicDemoSource::new("periodic_demo")
                    .with_interval(src_cfg.interval_ms)
                    .with_batch_size(src_cfg.batch_size)
                    .with_metric_names(src_cfg.metric_names.clone());
                bp.add_source("periodic_demo", s);
            },
            other => {
                tracing::warn!("unknown source type [{other}]");
            },
        }

        for (i, p) in demo_config.processors.iter().enumerate() {
            match p.processor_type.as_str() {
                "enrich" => {
                    bp.add_processor(
                        format!("enrich_{i}"),
                        EnrichProcessor::new("enrich", p.key.clone(), p.value.clone()),
                    );
                },
                "threshold_filter" => {
                    bp.add_processor(
                        format!("threshold_{i}"),
                        ThresholdFilter::new("threshold_filter", p.min_value),
                    );
                },
                "noop" => {
                    bp.add_processor(format!("noop_{i}"), NoopProcessor::new("noop"));
                },
                other => {
                    tracing::warn!("unknown processor type [{other}]");
                },
            }
        }

        let rep_cfg = demo_config.reporter.as_ref().expect("reporter required in simple mode");
        match rep_cfg.reporter_type.as_str() {
            "console" => {
                let fmt = match rep_cfg.format.as_str() {
                    "json" => ConsoleFormat::Json,
                    _ => ConsoleFormat::Summary,
                };
                let r = ConsoleReporter::new("console")
                    .with_format(fmt)
                    .with_log_interval(rep_cfg.log_interval_batches);
                signal_counter = r.signal_count();
                bp.add_reporter("console", r);
            },
            other => tracing::warn!("unknown reporter type [{other}]"),
        }
    }

    // ── 4. 从 YAML 构建 PipelineSpec 并 spawn ─────────────────────────
    let mut handles: Vec<PipelineHandle> = vec![];

    if is_dsl_mode {
        for (pipe_name, pipe_def) in &demo_config.pipelines {
            let spec = build_spec_from_def(pipe_name, pipe_def);
            tracing::info!(
                "🔗 Spawning pipeline [{pipe_name}]: {} sources, {} processors, {} reporters",
                pipe_def.sources.len(),
                pipe_def.processors.len(),
                pipe_def.reporters.len()
            );

            match bp.spawn(&spec) {
                Ok(h) => handles.push(h),
                Err(e) => {
                    tracing::error!("failed to spawn pipeline [{pipe_name}]: {e}");
                    tracing::info!(
                        "💡 hint: PipelineBlueprint::spawn() consumes components. \
                         Check that each ref: name is registered and not already used."
                    );
                },
            }
        }
    } else {
        let simple = demo_config.pipeline.as_ref().expect("pipeline required in simple mode");
        let spec = PipelineSpec {
            name: simple.name.clone(),
            channel_capacity: simple.channel_capacity,
            backpressure: to_backpressure(&simple.backpressure),
            ..Default::default()
        };
        handles.push(bp.spawn(&spec).expect("pipeline spawn failed"));
    }

    if handles.is_empty() {
        tracing::error!("no pipelines spawned — exiting");
        return;
    }
    tracing::info!("🔗 {} pipeline(s) spawned", handles.len());

    // ── 5. Scheduler 系统 ────────────────────────────────────────────
    app.add_system(
        Stage::Update,
        "config_watcher",
        |cfg: Cfg<DemoConfig>| -> zerotrace_core::error::Result<()> {
            if cfg.is_changed() {
                tracing::info!("config changed");
            }
            Ok(())
        },
    );

    // ── 6. 运行 ──────────────────────────────────────────────────────
    let run_duration = tokio::time::Duration::from_secs(10);
    tracing::info!("⏱️  Running for {run_duration:?}...");

    let runtime_handle = tokio::runtime::Handle::current();
    let shutdown = tokio::time::sleep(run_duration);
    tokio::pin!(shutdown);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            biased;
            _ = &mut ctrl_c => { tracing::info!("⚠️  SIGINT"); break; }
            _ = &mut shutdown => { tracing::info!("⏰ Done"); break; }
            _ = app.run_async(&runtime_handle) => {}
        }
    }

    // ── 7. 关闭 ──────────────────────────────────────────────────────
    for (i, h) in handles.iter_mut().enumerate() {
        h.shutdown();
        match h.shutdown_timeout(tokio::time::Duration::from_secs(2)).await {
            Ok(()) => tracing::info!("✅ pipeline[{i}] shut down"),
            Err(n) => tracing::warn!("⚠️  pipeline[{i}] {} tasks pending", n),
        }
    }

    // ── 报告 ──────────────────────────────────────────────────────────
    let signals = signal_counter.load(std::sync::atomic::Ordering::Relaxed);
    println!();
    println!("══════════════════════════════════════════════════════════");
    println!("  Demo Results");
    println!("══════════════════════════════════════════════════════════");
    println!(
        "  Mode:                {}",
        if is_dsl_mode { "ref: DSL" } else { "simple" }
    );
    println!("  Bundles loaded:      {:?}", loaded_bundles);
    println!("  Pipelines spawned:   {}", handles.len());
    println!("  Signals reported:    {signals}");
    println!("══════════════════════════════════════════════════════════");
    println!();
    tracing::info!("👋 Demo complete");
}

// ── 辅助函数 ──────────────────────────────────────────────────────────

fn to_backpressure(s: &str) -> BackpressurePolicy {
    match s {
        "drop_oldest" => BackpressurePolicy::DropOldest,
        "drop_newest" => BackpressurePolicy::DropNewest,
        _ => BackpressurePolicy::Block,
    }
}

/// 从 YAML PipelineDef 构建 PipelineSpec。
///
/// ref: 名字直接映射到 Blueprint 中注册的组件名。
/// Blueprint::spawn() 会按名查找并消费组件。
fn build_spec_from_def(name: &str, def: &zerotrace_demo::config::PipelineDef) -> PipelineSpec {
    let source_ids: Vec<String> = def.sources.iter().map(|s| s.ref_name.clone()).collect();
    let processor_ids: Vec<String> = def.processors.iter().map(|p| p.ref_name.clone()).collect();
    let reporter_ids: Vec<String> = def.reporters.iter().map(|r| r.ref_name.clone()).collect();

    PipelineSpec {
        name: name.into(),
        source_ids,
        processor_ids,
        reporter_ids,
        channel_capacity: def.channel_capacity,
        backpressure: to_backpressure(&def.backpressure),
        enabled: true,
    }
}
