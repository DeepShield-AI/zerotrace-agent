//! 应用层：YAML `config:` 字段 → PipelineBlueprint 工厂调用。
//!
//! PipelineBlueprint 只管名字匹配。config 覆写是应用层的事。
//! 本模块示范三种方式处理不同 endpoint 的共享 reporter。

use crate::config::DemoConfig;
use std::collections::HashMap;
use zerotrace_runtime::{blueprint::PipelineBlueprint, pipeline::BoxedReporter};

// ═══════════════════════════════════════════════════════════════════════
// 方案 1：注册两个模板（最直接）
// ═══════════════════════════════════════════════════════════════════════

/// 简单场景：已知有固定的几个 endpoint。
/// 每个 endpoint 注册一个共享模板即可。
pub fn register_reporters_by_endpoint(
    bp: &mut PipelineBlueprint,
    endpoints: &HashMap<String, String>,
) {
    for (name, url) in endpoints {
        bp.add_reporter_shared(
            name.clone(),
            crate::reporters::NoopReporter::new("http"), // 实际应用中是 HttpReporter
        );
        tracing::info!("registered shared reporter [{name}] → {url}");
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 方案 2：从 YAML config: 解析参数，动态创建
// ═══════════════════════════════════════════════════════════════════════
///
/// 对每条 pipeline，解析其 `reporters:` 中每个 `{ref, config}`：
/// - 如果 config 为 None → 使用共享模板（add_reporter_shared 已注册）
/// - 如果 config 为 Some → 创建配置好的实例，以唯一名注册为 consuming
///
/// 调用时机：在 `bp.spawn()` 之前。
pub fn resolve_pipeline_configs(
    bp: &mut PipelineBlueprint,
    config: &DemoConfig,
) -> Vec<PipelineConfigOverride> {
    let mut overrides: Vec<PipelineConfigOverride> = Vec::new();

    for (pipe_name, pipe_def) in &config.pipelines {
        let mut pipeline_overrides = PipelineConfigOverride {
            pipeline_name: pipe_name.clone(),
            reporters: Vec::new(),
        };

        for comp_ref in &pipe_def.reporters {
            if let Some(cfg) = &comp_ref.config {
                // 从 config 中提取 endpoint
                let endpoint =
                    cfg.get("endpoint").and_then(|v| v.as_str()).unwrap_or("https://default/api");
                let batch_size =
                    cfg.get("batch_size").and_then(|v| v.as_u64()).unwrap_or(500) as usize;

                // 创建配置好的实例，以唯一名注册
                let unique_name = format!("{}_{}", comp_ref.ref_name, pipe_name);
                let reporter = crate::reporters::NoopReporter::new("http");
                bp.add_reporter_boxed(unique_name.clone(), reporter.into());

                pipeline_overrides.reporters.push(ReporterMapping {
                    original_ref: comp_ref.ref_name.clone(),
                    unique_name,
                    endpoint: endpoint.to_string(),
                    batch_size,
                });
            }
        }

        overrides.push(pipeline_overrides);
    }

    overrides
}

/// 记录一条 pipeline 中每个 reporter 的 config 覆写。
pub struct PipelineConfigOverride {
    pub pipeline_name: String,
    pub reporters: Vec<ReporterMapping>,
}

pub struct ReporterMapping {
    pub original_ref: String, // YAML 中的 ref: 名字
    pub unique_name: String,  // 注册到 Blueprint 的唯一名
    pub endpoint: String,     // 覆写后的 endpoint
    pub batch_size: usize,
}

/// 将被覆写的 ref 名替换为唯一注册名。
pub fn apply_overrides(
    spec: &mut zerotrace_runtime::pipeline::PipelineSpec,
    overrides: &PipelineConfigOverride,
) {
    for mapping in &overrides.reporters {
        for id in &mut spec.reporter_ids {
            if *id == mapping.original_ref {
                *id = mapping.unique_name.clone();
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 方案 3：配置驱动的工厂（另一种思路）
// ═══════════════════════════════════════════════════════════════════════

/// 将 YAML config 解析逻辑封装在工厂里。
///
/// 用法：
/// ```ignore
/// bp.add_reporter_factory("http_forwarder", || {
///     // 从某个地方读取当前 pipeline 的 config...
///     BoxedReporter::from(create_http_reporter(endpoint, batch_size))
/// });
/// ```
///
/// 限制：工厂在 bp.spawn() 内部被调用，此时已无法访问 YAML config。
/// 所以工厂要么读全局状态，要么由调用者在注册工厂前把 config 注入闭包。
///
/// 推荐做法：方案 2（注册前解析 config）更清晰，逻辑在应用层完全可见。
pub fn register_factories_from_config(
    bp: &mut PipelineBlueprint,
    endpoints: Vec<(String, String, usize)>,
) {
    for (ref_name, endpoint, batch_size) in endpoints {
        bp.add_reporter_factory(ref_name, move || {
            let reporter = create_configured_reporter(&endpoint, batch_size);
            BoxedReporter::from(reporter)
        });
    }
}

fn create_configured_reporter(
    _endpoint: &str,
    _batch_size: usize,
) -> crate::reporters::NoopReporter {
    crate::reporters::NoopReporter::new("http")
    // 实际应用：HttpReporter::new(endpoint).with_batch_size(batch_size)
}

#[cfg(test)]
mod tests {
    use crate::config::DemoConfig;

    #[test]
    fn test_parse_config_with_endpoint_override() {
        let yaml = r#"
pipelines:
  metrics:
    sources: [{ref: cpu}]
    reporters:
      - ref: http_forwarder
        config:
          endpoint: "https://server/api/metrics"
          batch_size: 500
"#;
        let cfg = DemoConfig::from_str(yaml).unwrap();
        let pipe = &cfg.pipelines["metrics"];
        let rep = &pipe.reporters[0];
        assert_eq!(rep.ref_name, "http_forwarder");
        let ep = rep.config.as_ref().unwrap().get("endpoint").unwrap().as_str().unwrap();
        assert_eq!(ep, "https://server/api/metrics");
    }

    #[test]
    fn test_parse_config_without_override() {
        let yaml = r#"
pipelines:
  metrics:
    sources: [{ref: cpu}]
    reporters: [{ref: http_forwarder}]
"#;
        let cfg = DemoConfig::from_str(yaml).unwrap();
        let pipe = &cfg.pipelines["metrics"];
        assert!(pipe.reporters[0].config.is_none());
        // config 为 None → 使用共享模板的默认配置
    }
}
