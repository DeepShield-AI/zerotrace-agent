//! 集成测试 — 演示 5 个测试层级。
//!
//! ```
//! L1: 组件单元测试    — 各模块 #[cfg(test)]（不在这里）
//! L2: Bundle 依赖链   — bundles::tests（不在这里）
//! L3: Pipeline 集成   — 本文件：Mock Source + 真实 Processor + CollectingReporter
//! L4: Blueprint 测试  — 本文件：PipelineBlueprint 注册/消费/错误
//! L5: E2E             — 本文件：YAML 驱动全链路
//! ```

use std::sync::Arc;
use zerotrace_core::signal::{Batch, BatchMetadata};
use zerotrace_demo::{
    bundles::{self, AgentSession, ForwarderHandle},
    collectors::MockDemoSource,
    config::DemoConfig,
    processors::{EnrichProcessor, SpyProcessor, ThresholdFilter},
    signals::DemoMetric,
};
use zerotrace_kernel::{bundle::BundleSet, world::World};
use zerotrace_runtime::{
    blueprint::PipelineBlueprint,
    pipeline::{
        BackpressurePolicy, CollectingReporter, IterSource, PipelineExecutor, PipelineSpec,
    },
};

// ═══════════════════════════════════════════════════════════════════════
// 辅助函数
// ═══════════════════════════════════════════════════════════════════════

fn make_metrics() -> Vec<DemoMetric> {
    vec![
        DemoMetric::gauge("cpu.utilization", 85.0, 1000).with_label("host", "n1"),
        DemoMetric::gauge("memory.usage_bytes", 4_200_000_000.0, 1000).with_label("host", "n1"),
        DemoMetric::gauge("disk.io_ops", -1.0, 1000), // 会被阈值过滤
    ]
}

fn make_batch(metrics: Vec<DemoMetric>) -> Batch {
    let meta = Arc::new(BatchMetadata::new("test"));
    let mut batch = Batch::new(meta);
    for m in metrics {
        batch.push(m);
    }
    batch
}

// ═══════════════════════════════════════════════════════════════════════
// L3: Pipeline 集成测试 — 用 IterSource + 真实 Processor + CollectingReporter
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_pipeline_enrich_chain() {
    let source = IterSource::new("src", vec![make_batch(make_metrics())]);
    let enrich = EnrichProcessor::new("enrich", "env", "integration_test");
    let reporter = CollectingReporter::new("collector");
    let received = reporter.batches.clone();

    let spec = PipelineSpec {
        name: "test".into(),
        channel_capacity: 16,
        ..Default::default()
    };
    let handle = PipelineExecutor::spawn(
        &spec,
        vec![("s1".into(), source.into())],
        vec![("p1".into(), enrich.into())],
        vec![("r1".into(), reporter.into())],
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    handle.shutdown();

    let batches = received.lock();
    assert_eq!(batches.len(), 1);

    let metrics: Vec<&DemoMetric> = batches[0].filter::<DemoMetric>();
    assert_eq!(metrics.len(), 3);
    for m in &metrics {
        assert!(
            m.labels.iter().any(|(k, v)| k == "env" && v == "integration_test"),
            "metric {} missing env label",
            m.name
        );
    }
}

#[tokio::test]
async fn test_pipeline_enrich_then_filter_drops_negative_values() {
    let source = IterSource::new("src", vec![make_batch(make_metrics())]);
    let enrich = EnrichProcessor::new("enrich", "env", "test");
    let filter = ThresholdFilter::new("filter", 0.0);
    let reporter = CollectingReporter::new("collector");
    let received = reporter.batches.clone();

    let spec = PipelineSpec {
        name: "test".into(),
        channel_capacity: 32,
        ..Default::default()
    };
    let handle = PipelineExecutor::spawn(
        &spec,
        vec![("s1".into(), source.into())],
        vec![
            ("p_enrich".into(), enrich.into()),
            ("p_filter".into(), filter.into()),
        ],
        vec![("r1".into(), reporter.into())],
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    handle.shutdown();

    let batches = received.lock();
    let metrics: Vec<&DemoMetric> = batches.iter().flat_map(|b| b.filter::<DemoMetric>()).collect();
    // disk.io_ops = -1.0 应被过滤
    assert_eq!(metrics.len(), 2);
    assert!(metrics.iter().all(|m| m.value >= 0.0));
}

#[tokio::test]
async fn test_pipeline_multiple_batches_and_sources() {
    let batch1 = make_batch(vec![DemoMetric::gauge("a", 1.0, 0)]);
    let batch2 = make_batch(vec![DemoMetric::gauge("b", 2.0, 0)]);
    let batch3 = make_batch(vec![DemoMetric::gauge("c", 3.0, 0)]);

    let source1 = IterSource::new("s1", vec![batch1, batch2]);
    let source2 = IterSource::new("s2", vec![batch3]);
    let reporter = CollectingReporter::new("collector");
    let received = reporter.batches.clone();

    let spec = PipelineSpec {
        name: "multi".into(),
        channel_capacity: 16,
        ..Default::default()
    };
    let handle = PipelineExecutor::spawn(
        &spec,
        vec![("s1".into(), source1.into()), ("s2".into(), source2.into())],
        vec![],
        vec![("r1".into(), reporter.into())],
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    handle.shutdown();

    let batches = received.lock();
    let total: usize = batches.iter().map(|b| b.len()).sum();
    assert_eq!(total, 3);
}

// ═══════════════════════════════════════════════════════════════════════
// L3: Pipeline 集成测试 — 用 Mock 自定义 Source
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_pipeline_with_mock_source_and_spy_processor() {
    let source = MockDemoSource::from_metrics(
        "mock_src",
        vec![
            DemoMetric::gauge("cpu", 85.0, 1000),
            DemoMetric::gauge("mem", 42.0, 2000),
        ],
    );
    let spy = SpyProcessor::new("spy");
    let reporter = CollectingReporter::new("collector");
    let received = reporter.batches.clone();

    let spec = PipelineSpec {
        name: "mock".into(),
        channel_capacity: 16,
        ..Default::default()
    };
    let handle = PipelineExecutor::spawn(
        &spec,
        vec![("s1".into(), source.into())],
        vec![("spy".into(), spy.into())],
        vec![("r1".into(), reporter.into())],
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    handle.shutdown();

    let batches = received.lock();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 2);
    // Spy 已被消费，无法再读取 call_count — 但我们可以验证数据到达
}

// ═══════════════════════════════════════════════════════════════════════
// L3: 背压策略测试
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_pipeline_block_backpressure_no_data_loss() {
    // 小 channel + 慢 processor → Block 模式确保零丢失
    let batches: Vec<Batch> =
        (0..20).map(|i| make_batch(vec![DemoMetric::gauge("m", i as f64, 0)])).collect();
    let source = IterSource::new("src", batches);
    let reporter = CollectingReporter::new("collector");
    let received = reporter.batches.clone();

    let spec = PipelineSpec {
        name: "block".into(),
        channel_capacity: 4, // 小容量
        backpressure: BackpressurePolicy::Block,
        ..Default::default()
    };
    let handle = PipelineExecutor::spawn(
        &spec,
        vec![("s1".into(), source.into())],
        vec![],
        vec![("r1".into(), reporter.into())],
    );

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    handle.shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Block 模式：零丢失
    assert_eq!(received.lock().len(), 20);
}

// ═══════════════════════════════════════════════════════════════════════
// L4: PipelineBlueprint 测试
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_blueprint_consumes_components() {
    let mut bp = PipelineBlueprint::new();
    bp.add_source("s1", IterSource::new("s1", vec![]));
    assert!(bp.has_source("s1"));
    assert_eq!(bp.source_count(), 1);

    // spawn 消费组件
    let spec = PipelineSpec {
        name: "consuming".into(),
        source_ids: vec!["s1".into()],
        ..Default::default()
    };
    let _handle = bp.spawn(&spec).unwrap();
    assert!(!bp.has_source("s1"));
    assert_eq!(bp.source_count(), 0);
}

#[tokio::test]
async fn test_blueprint_double_spawn_errors() {
    let mut bp = PipelineBlueprint::new();
    bp.add_source("s1", IterSource::new("s1", vec![]));
    bp.add_reporter("r1", CollectingReporter::new("r1"));

    let spec = PipelineSpec {
        name: "once".into(),
        source_ids: vec!["s1".into()],
        reporter_ids: vec!["r1".into()],
        ..Default::default()
    };
    let _handle = bp.spawn(&spec).unwrap();

    let result = bp.spawn(&spec);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("already consumed") || msg.contains("not registered"),
        "expected consumption error, got: {msg}"
    );
}

#[test]
fn test_blueprint_missing_component_error() {
    let mut bp = PipelineBlueprint::new();
    let spec = PipelineSpec {
        name: "missing".into(),
        source_ids: vec!["nonexistent".into()],
        ..Default::default()
    };
    let _result = bp.spawn(&spec);
    assert!(_result.is_err());
    assert!(_result.unwrap_err().to_string().contains("nonexistent"));
}

// ═══════════════════════════════════════════════════════════════════════
// L2: Bundle 依赖链（交叉验证 bundles.rs 中的测试）
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_bundle_dependency_chain_integration() {
    let world = World::new();
    let mut set = BundleSet::new(&world);

    let session = Arc::new(parking_lot::RwLock::new(AgentSession {
        server_url: "s".into(),
        connected: true,
    }));

    // 加载顺序：Session（无依赖）→ Forwarder（依赖 Session）
    // Forwarder 先于 Session 加载会失败
    set.load(&bundles::SessionBundle {
        session: session.clone(),
    })
    .unwrap();
    set.load(&bundles::ForwarderBundle {
        forwarder: Arc::new(parking_lot::RwLock::new(ForwarderHandle {
            session: session.clone(),
            endpoint: "/test".into(),
        })),
    })
    .unwrap();

    assert!(world.contains::<AgentSession>());
    assert!(world.contains::<ForwarderHandle>());
}

// ═══════════════════════════════════════════════════════════════════════
// L5: E2E — YAML 驱动全链路
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_yaml_config_roundtrip() {
    // 验证 YAML → struct → 可用的完整链路
    let yaml = r#"
version: 1
bundles: [core, pipeline]
pipeline:
  name: "e2e-test"
  channel_capacity: 64
  backpressure: "block"
source:
  type: "periodic_demo"
  interval_ms: 100
  batch_size: 2
  metric_names: ["test.metric"]
processors:
  - type: "enrich"
    key: "test_key"
    value: "test_val"
reporter:
  type: "console"
  format: "summary"
  log_interval_batches: 10
"#;

    let cfg = DemoConfig::from_str(yaml).unwrap();
    assert_eq!(cfg.version, 1);
    assert_eq!(cfg.bundles, vec!["core", "pipeline"]);
    let pipe = cfg.pipeline.as_ref().unwrap();
    assert_eq!(pipe.name, "e2e-test");
    let src = cfg.source.as_ref().unwrap();
    assert_eq!(src.interval_ms, 100);
    assert_eq!(src.metric_names, vec!["test.metric"]);
    assert_eq!(cfg.processors.len(), 1);
    assert_eq!(cfg.processors[0].key, "test_key");
    let rep = cfg.reporter.as_ref().unwrap();
    assert_eq!(rep.format, "summary");
}

#[tokio::test]
async fn test_e2e_yaml_driven_small_pipeline() {
    // 用 YAML 配置构造最小管线，跑几个 batch，验证数据流

    let config = DemoConfig {
        version: 1,
        bundles: vec!["core".into()],
        pipeline: Some(zerotrace_demo::config::SimplePipelineConfig {
            name: "e2e-small".into(),
            channel_capacity: 16,
            backpressure: "block".into(),
        }),
        source: Some(zerotrace_demo::config::SourceConfig {
            source_type: "periodic_demo".into(),
            interval_ms: 30,
            batch_size: 2,
            metric_names: vec!["e2e.counter".into()],
        }),
        processors: vec![zerotrace_demo::config::ProcessorConfig {
            processor_type: "enrich".into(),
            key: "e2e".into(),
            value: "true".into(),
            min_value: 0.0,
        }],
        reporter: Some(zerotrace_demo::config::ReporterConfig {
            reporter_type: "console".into(),
            format: "summary".into(),
            log_interval_batches: 100,
        }),
        pipelines: std::collections::HashMap::new(),
    };

    let src_cfg = config.source.as_ref().unwrap();
    let pipe_cfg = config.pipeline.as_ref().unwrap();

    // 用 YAML 驱动选择组件
    let source = zerotrace_demo::collectors::PeriodicDemoSource::new("periodic_demo")
        .with_interval(src_cfg.interval_ms)
        .with_batch_size(src_cfg.batch_size)
        .with_metric_names(src_cfg.metric_names.clone());

    let enrich = EnrichProcessor::new(
        "enrich",
        config.processors[0].key.clone(),
        config.processors[0].value.clone(),
    );

    let reporter = CollectingReporter::new("collector");
    let received = reporter.batches.clone();

    let mut bp = PipelineBlueprint::new();
    bp.add_source("s1", source);
    bp.add_processor("p1", enrich);
    bp.add_reporter("r1", reporter);

    let spec = PipelineSpec {
        name: pipe_cfg.name.clone(),
        channel_capacity: pipe_cfg.channel_capacity,
        ..Default::default()
    };

    let handle = bp.spawn(&spec).unwrap();

    // 等几批数据
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    handle.shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let batches = received.lock();
    assert!(
        !batches.is_empty(),
        "should have received at least one batch"
    );

    // 验证每个信号都有 e2e=true 标签（enrich processor 生效）
    for batch in batches.iter() {
        for item in &batch.items {
            if let Some(m) = item.downcast::<DemoMetric>() {
                assert!(
                    m.labels.iter().any(|(k, v)| k == "e2e" && v == "true"),
                    "metric {} missing e2e=true label",
                    m.name
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// L2: App + World 容器级测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_world_resource_injection() {
    let world = World::new();
    let config = DemoConfig::default();
    world.insert(config.clone());

    let (lock, _) = world.get::<DemoConfig>().unwrap();
    assert_eq!(lock.read().version, 1);
    assert!(!lock.read().bundles.is_empty());
}

#[test]
fn test_world_resource_change_detection() {
    let world = World::new();

    // 用普通的 struct（insert 自动包在 Arc<RwLock<>> 中）
    world.insert(DemoConfig::default());

    let (_, meta1) = world.get::<DemoConfig>().unwrap();
    let tick1 = meta1.changed_tick;

    // 再次插入同类型 → change_tick 递增（hot-reload 模式）
    world.insert(DemoConfig {
        version: 99,
        ..DemoConfig::default()
    });

    let (_, meta2) = world.get::<DemoConfig>().unwrap();
    assert!(
        meta2.changed_tick > tick1,
        "change tick should advance on overwrite insert"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// L2: Bundle 拓扑排序交叉验证
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_load_all_handles_reverse_registration_order() {
    let world = World::new();
    let mut set = BundleSet::new(&world);

    let session = Arc::new(parking_lot::RwLock::new(AgentSession {
        server_url: "s".into(),
        connected: true,
    }));
    let _config = Arc::new(parking_lot::RwLock::new(DemoConfig::default()));

    let session_bundle = bundles::SessionBundle {
        session: session.clone(),
    };
    let forwarder = bundles::ForwarderBundle {
        forwarder: Arc::new(parking_lot::RwLock::new(ForwarderHandle {
            session: session.clone(),
            endpoint: "/test".into(),
        })),
    };

    // 故意倒序：Forwarder（依赖 Session）在前，Session 在后
    // load_all 应自动纠正
    set.load_all(&[
        &forwarder as &dyn zerotrace_kernel::bundle::Bundle,
        &session_bundle as &dyn zerotrace_kernel::bundle::Bundle,
    ])
    .unwrap();

    assert!(world.contains::<AgentSession>());
    assert!(world.contains::<ForwarderHandle>());
}

// ═══════════════════════════════════════════════════════════════════════
// ref: DSL — 多 pipeline + 引用解析 + 消费语义
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_ref_dsl_multi_pipeline_spawn() {
    let mut bp = PipelineBlueprint::new();

    bp.add_source(
        "host_metrics",
        MockDemoSource::from_metrics("host", vec![DemoMetric::gauge("cpu", 80.0, 0)]),
    );
    bp.add_processor("tag", EnrichProcessor::new("tag", "pipeline", "metrics"));
    let rep = CollectingReporter::new("rep");
    let received1 = rep.batches.clone();
    bp.add_reporter("console_metrics", rep);

    bp.add_source(
        "l7_source",
        MockDemoSource::from_metrics("l7", vec![DemoMetric::gauge("l7.latency", 5.0, 0)]),
    );
    let rep2 = CollectingReporter::new("rep2");
    let received2 = rep2.batches.clone();
    bp.add_reporter("console_l7", rep2);

    let h1 = bp
        .spawn(&PipelineSpec {
            name: "metrics".into(),
            source_ids: vec!["host_metrics".into()],
            processor_ids: vec!["tag".into()],
            reporter_ids: vec!["console_metrics".into()],
            channel_capacity: 64,
            ..Default::default()
        })
        .unwrap();
    let h2 = bp
        .spawn(&PipelineSpec {
            name: "l7".into(),
            source_ids: vec!["l7_source".into()],
            processor_ids: vec![],
            reporter_ids: vec!["console_l7".into()],
            channel_capacity: 32,
            ..Default::default()
        })
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    h1.shutdown();
    h2.shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let lock1 = received1.lock();
    let lock2 = received2.lock();
    assert_eq!(lock1.len(), 1);
    assert_eq!(lock2.len(), 1);
    let m: Vec<&DemoMetric> = lock1[0].filter::<DemoMetric>();
    assert!(m[0].labels.iter().any(|(k, v)| k == "pipeline" && v == "metrics"));
}

#[tokio::test]
async fn test_ref_dsl_consuming_semantics_error() {
    let mut bp = PipelineBlueprint::new();
    bp.add_source("only_once", MockDemoSource::from_metrics("s", vec![]));
    bp.add_reporter("r", CollectingReporter::new("r"));

    let spec = PipelineSpec {
        name: "first".into(),
        source_ids: vec!["only_once".into()],
        reporter_ids: vec!["r".into()],
        ..Default::default()
    };
    let _h = bp.spawn(&spec).unwrap();

    let _result = bp.spawn(&spec);
    assert!(_result.is_err());
    let msg = _result.unwrap_err().to_string();
    assert!(
        msg.contains("already consumed") || msg.contains("not registered"),
        "expected consumption error, got: {msg}"
    );
}

#[test]
fn test_ref_dsl_manifest_parses_full_multi_pipeline() {
    let yaml = r#"
bundles: [core, host_metric, ebpf]
pipelines:
  metrics:
    sources: [{ref: host_metric_collector}]
    processors: [{ref: tagging}, {ref: filter, config: {min_value: 0.5}}]
    reporters: [{ref: http_forwarder, config: {batch_size: 500, timeout_ms: 10000}}]
    channel_capacity: 4096
    backpressure: drop_oldest
  l7:
    sources: [{ref: ebpf_socket_collector}]
    processors: [{ref: reorder}, {ref: reassembly}, {ref: l7_parse}, {ref: trace_assembly}]
    reporters: [{ref: http_forwarder}]
"#;
    let cfg = DemoConfig::from_str(yaml).unwrap();
    assert_eq!(cfg.bundles, vec!["core", "host_metric", "ebpf"]);
    assert_eq!(cfg.pipelines.len(), 2);

    let metrics = &cfg.pipelines["metrics"];
    assert_eq!(metrics.sources[0].ref_name, "host_metric_collector");
    assert_eq!(metrics.processors[1].ref_name, "filter");
    assert!(metrics.processors[1].config.is_some());
    assert_eq!(metrics.reporters[0].ref_name, "http_forwarder");
    assert_eq!(metrics.backpressure, "drop_oldest");

    let l7 = &cfg.pipelines["l7"];
    assert_eq!(l7.processors.len(), 4);
    assert_eq!(l7.processors[3].ref_name, "trace_assembly");
    assert_eq!(l7.channel_capacity, 1024); // default
}
