//! YAML-driven configuration types.
//!
//! # 两种模式
//!
//! **简单模式** (当前 demo): 单 pipeline + `type:` 工厂模式
//! ```yaml
//! pipeline: {name: "demo", channel_capacity: 1024}
//! source: {type: "periodic_demo", interval_ms: 500}
//! ```
//! → main.rs 用 `match type` 创建具体类型
//!
//! **ref: DSL 模式** (ADR-005 设计目标): 多 pipeline + `ref:` 引用模式
//! ```yaml
//! pipelines:
//!   metrics:
//!     sources: [{ref: host_metric_collector}]
//!     processors: [{ref: tagging}]
//!     reporters: [{ref: http_forwarder, config: {batch_size: 500}}]
//! ```
//! → Bundle 注册组件到 Blueprint，YAML 只管接线
//!
//! # 设计原则
//!
//! | 层级 | 配置项 | 表达方式 | 谁控制 |
//! |------|--------|---------|-------|
//! | L3 | `bundles:` | YAML 列表 | 运维/SRE |
//! | L4 | `pipelines:` | YAML `ref:` DSL | 运维/SRE |
//! | - | Bundle 内部组件依赖 | TypeId | 编译期 |
//! | - | `ref:` → 组件名解析 | Blueprint::spawn() | 运行时 |

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 顶层配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemoConfig {
    #[serde(default = "default_version")]
    pub version: u32,

    /// L3 — 加载哪些 Bundle。
    #[serde(default = "default_bundles")]
    pub bundles: Vec<String>,

    // ── 简单模式（向后兼容） ──
    /// 单 pipeline 配置（与 `pipelines:` 互斥）。
    #[serde(default)]
    pub pipeline: Option<SimplePipelineConfig>,

    /// Source 参数（简单模式）。
    #[serde(default)]
    pub source: Option<SourceConfig>,

    /// Processor 链参数（简单模式）。
    #[serde(default)]
    pub processors: Vec<ProcessorConfig>,

    /// Reporter 参数（简单模式）。
    #[serde(default)]
    pub reporter: Option<ReporterConfig>,

    // ── ref: DSL 模式（多 pipeline） ──
    /// 多 pipeline 定义。Key = pipeline 名称。
    #[serde(default)]
    pub pipelines: HashMap<String, PipelineDef>,
}

fn default_version() -> u32 {
    1
}
fn default_bundles() -> Vec<String> {
    vec!["demo_core".into()]
}

// ═══════════════════════════════════════════════════════════════════════
// 简单模式（type: 工厂）
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimplePipelineConfig {
    #[serde(default = "default_pipeline_name")]
    pub name: String,
    #[serde(default = "default_channel_capacity")]
    pub channel_capacity: usize,
    #[serde(default = "default_backpressure")]
    pub backpressure: String,
}

fn default_pipeline_name() -> String {
    "demo-pipeline".into()
}
fn default_channel_capacity() -> usize {
    1024
}
fn default_backpressure() -> String {
    "block".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_metric_names")]
    pub metric_names: Vec<String>,
}

fn default_interval_ms() -> u64 {
    1000
}
fn default_batch_size() -> usize {
    3
}
fn default_metric_names() -> Vec<String> {
    vec![
        "demo.metric.1".into(),
        "demo.metric.2".into(),
        "demo.metric.3".into(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessorConfig {
    #[serde(rename = "type")]
    pub processor_type: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub min_value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReporterConfig {
    #[serde(rename = "type")]
    pub reporter_type: String,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_log_interval")]
    pub log_interval_batches: usize,
}

fn default_format() -> String {
    "summary".into()
}
fn default_log_interval() -> usize {
    5
}

// ═══════════════════════════════════════════════════════════════════════
// ref: DSL 模式（多 pipeline）
// ═══════════════════════════════════════════════════════════════════════

/// 一个管线阶段的组件引用。
///
/// ```yaml
/// sources:
///   - ref: host_metric_collector
///   - ref: ebpf_socket_collector
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentRef {
    /// 组件在 PipelineBlueprint 中的注册名。
    #[serde(rename = "ref")]
    pub ref_name: String,

    /// 可选的在线配置覆写（对应该组件的 config schema）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_yaml::Value>,
}

/// 一条管线的完整定义。
///
/// ```yaml
/// metrics_pipeline:
///   sources: [{ref: host_metric_collector}]
///   processors: [{ref: metrics_aggregator}, {ref: tagging}]
///   reporters: [{ref: http_forwarder, config: {batch_size: 500}}]
///   channel_capacity: 4096
///   backpressure: block
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDef {
    pub sources: Vec<ComponentRef>,
    #[serde(default)]
    pub processors: Vec<ComponentRef>,
    #[serde(default)]
    pub reporters: Vec<ComponentRef>,
    #[serde(default = "default_channel_capacity")]
    pub channel_capacity: usize,
    #[serde(default = "default_backpressure")]
    pub backpressure: String,
}

// ═══════════════════════════════════════════════════════════════════════
// 构造器
// ═══════════════════════════════════════════════════════════════════════

impl DemoConfig {
    pub fn from_file(path: &str) -> Result<Self, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read [{path}]: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("YAML parse error in [{path}]: {e}"))
    }

    pub fn from_str(yaml: &str) -> Result<Self, String> {
        serde_yaml::from_str(yaml).map_err(|e| format!("YAML parse error: {e}"))
    }

    /// 是否有 ref: DSL 模式的多 pipeline 定义。
    pub fn has_pipelines(&self) -> bool {
        !self.pipelines.is_empty()
    }

    /// 是否有简单模式的单 pipeline 定义。
    pub fn has_simple_pipeline(&self) -> bool {
        self.pipeline.is_some() && self.source.is_some() && self.reporter.is_some()
    }
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            version: 1,
            bundles: default_bundles(),
            pipeline: Some(SimplePipelineConfig {
                name: default_pipeline_name(),
                channel_capacity: default_channel_capacity(),
                backpressure: default_backpressure(),
            }),
            source: Some(SourceConfig {
                source_type: "periodic_demo".into(),
                interval_ms: default_interval_ms(),
                batch_size: default_batch_size(),
                metric_names: default_metric_names(),
            }),
            processors: vec![ProcessorConfig {
                processor_type: "enrich".into(),
                key: "environment".into(),
                value: "demo".into(),
                min_value: 0.0,
            }],
            reporter: Some(ReporterConfig {
                reporter_type: "console".into(),
                format: default_format(),
                log_interval_batches: default_log_interval(),
            }),
            pipelines: HashMap::new(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── 简单模式 ──────────────────────────────────────────────────

    #[test]
    fn parse_simple_mode() {
        let yaml = r#"
pipeline: {name: "test"}
source: {type: "periodic_demo"}
reporter: {type: "console"}
"#;
        let cfg: DemoConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.has_simple_pipeline());
        assert!(!cfg.has_pipelines());
    }

    // ── ref: DSL 模式 ──────────────────────────────────────────────

    #[test]
    fn parse_multi_pipeline_ref_dsl() {
        let yaml = r#"
bundles: [core, host_metric]
pipelines:
  metrics_pipeline:
    sources: [{ref: host_metric_collector}]
    processors: [{ref: tagging}]
    reporters: [{ref: http_forwarder, config: {batch_size: 500}}]
    channel_capacity: 4096
  l7_pipeline:
    sources: [{ref: ebpf_socket_collector}]
    processors: [{ref: reorder}, {ref: reassembly}, {ref: l7_parse}]
    reporters: [{ref: http_forwarder}]
"#;
        let cfg: DemoConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.has_pipelines());
        assert!(!cfg.has_simple_pipeline());
        assert_eq!(cfg.pipelines.len(), 2);

        let metrics = &cfg.pipelines["metrics_pipeline"];
        assert_eq!(metrics.sources[0].ref_name, "host_metric_collector");
        assert_eq!(metrics.channel_capacity, 4096);
        assert!(metrics.reporters[0].config.is_some());

        let l7 = &cfg.pipelines["l7_pipeline"];
        assert_eq!(l7.processors.len(), 3);
        assert_eq!(l7.processors[0].ref_name, "reorder");
        assert_eq!(l7.processors[2].ref_name, "l7_parse");
    }

    #[test]
    fn parse_ref_dsl_defaults() {
        let yaml = r#"
pipelines:
  minimal:
    sources: [{ref: s1}]
    reporters: [{ref: r1}]
"#;
        let cfg: DemoConfig = serde_yaml::from_str(yaml).unwrap();
        let p = &cfg.pipelines["minimal"];
        assert_eq!(p.channel_capacity, 1024); // default
        assert_eq!(p.backpressure, "block"); // default
        assert!(p.processors.is_empty());
    }

    #[test]
    fn parse_backpressure_mapping() {
        let yaml = r#"
pipelines:
  bp_test:
    sources: [{ref: s}]
    reporters: [{ref: r}]
    backpressure: drop_oldest
"#;
        let cfg: DemoConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.pipelines["bp_test"].backpressure, "drop_oldest");
    }

    #[test]
    fn parse_component_ref_without_config() {
        let yaml = r#"
pipelines:
  p:
    sources: [{ref: cpu_collector}]
    reporters: [{ref: console}]
"#;
        let cfg: DemoConfig = serde_yaml::from_str(yaml).unwrap();
        let p = &cfg.pipelines["p"];
        assert!(p.sources[0].config.is_none());
        assert!(p.reporters[0].config.is_none());
    }
}
