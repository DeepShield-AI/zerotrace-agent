//! Bundle 定义 — 编译期 TypeId 依赖 + 可选 YAML 管线模板。
//!
//! # 设计原则
//!
//! - **Bundle 内部依赖**：用 `#[component(deps = [TypeA, TypeB])]` 表达
//!   — 编译期 TypeId，`BundleSet::load_all()` 自动拓扑排序
//! - **Bundle 是否加载**：用 YAML `bundles: [core, host_metric]` 控制
//!   — 运维决策，不需要重新编译
//! - **管线拓扑**：用 YAML `pipelines:` 声明 Source→Processor→Reporter
//!   连接方式 — 运维决策
//!
//! # 模拟真实场景
//!
//! ```text
//! SessionBundle    -- 提供 AgentSession (基础设施)
//! ConfigBundle     -- 提供 DemoConfig (配置)
//! CollectorBundle  -- 提供 CollectorHandle (依赖 AgentSession + DemoConfig)
//! ForwarderBundle  -- 提供 ForwarderHandle (依赖 AgentSession)
//! ```

use crate::config::DemoConfig;
use parking_lot::RwLock;
use std::sync::Arc;
use zerotrace_kernel::bundle::{Bundle, ComponentDescriptor, PipelineTemplate};
use zerotrace_kernel_derive::Bundle;

// ═══════════════════════════════════════════════════════════════════════
// 模拟真实组件类型（模拟 agent 中的 Session, Forwarder, Collector）
// ═══════════════════════════════════════════════════════════════════════

/// 模拟 Agent 到 server 的连接会话（基础设施）。
#[derive(Debug)]
pub struct AgentSession {
    pub server_url: String,
    pub connected: bool,
}

/// 模拟数据上报器句柄。
#[derive(Debug)]
pub struct ForwarderHandle {
    pub session: Arc<RwLock<AgentSession>>,
    pub endpoint: String,
}

/// 模拟采集器句柄。
#[derive(Debug)]
pub struct CollectorHandle {
    pub session: Arc<RwLock<AgentSession>>,
    pub config: Arc<RwLock<DemoConfig>>,
    pub running: bool,
}

// ═══════════════════════════════════════════════════════════════════════
// Bundle 1: SessionBundle — 基础设施，无依赖
// ═══════════════════════════════════════════════════════════════════════

/// 基础设施 Bundle：提供 `AgentSession`。
///
/// `deps = []` — 无依赖，最先加载。
#[derive(Bundle)]
#[bundle(id = "session", name = "Agent Session")]
pub struct SessionBundle {
    #[component(id = "session", deps = [])]
    pub session: Arc<RwLock<AgentSession>>,
}

// ═══════════════════════════════════════════════════════════════════════
// Bundle 2: ConfigBundle — 配置，无依赖
// ═══════════════════════════════════════════════════════════════════════

#[derive(Bundle)]
#[bundle(id = "demo_config", name = "Demo Configuration")]
pub struct ConfigBundle {
    #[component(id = "config", deps = [])]
    pub config: Arc<RwLock<DemoConfig>>,
}

// ═══════════════════════════════════════════════════════════════════════
// Bundle 3: ForwarderBundle — 依赖 AgentSession（TypeId 检查）
// ═══════════════════════════════════════════════════════════════════════

/// 数据面上报 Bundle：依赖 `AgentSession`。
///
/// 加载时自动检查：World 中是否有 `TypeId::of::<AgentSession>()`。
/// 没有 → `BundleSet::load()` 返回 `Err(MissingDep)`。
#[derive(Bundle)]
#[bundle(id = "forwarder", name = "HTTP Forwarder")]
pub struct ForwarderBundle {
    #[component(id = "forwarder", deps = [AgentSession])]
    pub forwarder: Arc<RwLock<ForwarderHandle>>,
}

// ═══════════════════════════════════════════════════════════════════════
// Bundle 4: CollectorBundle — 依赖 AgentSession + DemoConfig
// ═══════════════════════════════════════════════════════════════════════

/// 采集 Bundle：同时依赖 `AgentSession` 和 `DemoConfig`。
///
/// 只有两个依赖都满足时才能加载。拓扑排序保证 SessionBundle 和
/// ConfigBundle 在本 Bundle 之前加载。
#[derive(Bundle)]
#[bundle(id = "collector", name = "Host Collector")]
pub struct CollectorBundle {
    #[component(id = "collector", deps = [AgentSession, DemoConfig])]
    pub collector: Arc<RwLock<CollectorHandle>>,
}

// ═══════════════════════════════════════════════════════════════════════
// Bundle 5: DemoPipelineBundle — 手动实现，仅声明管线模板
// ═══════════════════════════════════════════════════════════════════════

/// 纯管线模板 Bundle：零组件，只提供 default_pipelines()。
///
/// 对于零组件的 Bundle，手动 impl 比 `#[derive(Bundle)]` 更简洁。
#[allow(dead_code)]
pub struct DemoPipelineBundle;

impl Bundle for DemoPipelineBundle {
    fn id(&self) -> &'static str {
        "demo_pipeline"
    }
    fn name(&self) -> &'static str {
        "Demo Pipeline Template"
    }
    fn components(&self) -> Vec<ComponentDescriptor> {
        vec![]
    }
    fn default_pipelines(&self) -> Vec<PipelineTemplate> {
        vec![PipelineTemplate {
            name: "main".into(),
            sources: vec!["periodic_demo".into()],
            processors: vec![
                "enrich_env".into(),
                "enrich_collector".into(),
                "threshold".into(),
            ],
            reporters: vec!["console".into()],
        }]
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use zerotrace_kernel::{bundle::BundleSet, world::World};

    // ── 辅助函数 ──────────────────────────────────────────────────

    fn make_session_bundle() -> SessionBundle {
        SessionBundle {
            session: Arc::new(RwLock::new(AgentSession {
                server_url: "https://server:3000".into(),
                connected: true,
            })),
        }
    }

    fn make_config_bundle() -> ConfigBundle {
        ConfigBundle {
            config: Arc::new(RwLock::new(DemoConfig::default())),
        }
    }

    fn make_forwarder_bundle(session: Arc<RwLock<AgentSession>>) -> ForwarderBundle {
        ForwarderBundle {
            forwarder: Arc::new(RwLock::new(ForwarderHandle {
                session,
                endpoint: "/api/v1/data/ingest".into(),
            })),
        }
    }

    fn make_collector_bundle(
        session: Arc<RwLock<AgentSession>>,
        config: Arc<RwLock<DemoConfig>>,
    ) -> CollectorBundle {
        CollectorBundle {
            collector: Arc::new(RwLock::new(CollectorHandle {
                session,
                config,
                running: false,
            })),
        }
    }

    // ── 测试：TypeId 依赖链 ──────────────────────────────────────

    #[test]
    fn test_session_bundle_loads_standalone() {
        let world = World::new();
        let mut set = BundleSet::new(&world);
        set.load(&make_session_bundle()).unwrap();
        assert!(world.contains::<AgentSession>());
    }

    #[test]
    fn test_forwarder_fails_without_session() {
        let world = World::new();
        let mut set = BundleSet::new(&world);
        // 不加载 SessionBundle → ForwarderBundle 应失败
        let result = set.load(&make_forwarder_bundle(Arc::new(RwLock::new(
            AgentSession {
                server_url: String::new(),
                connected: false,
            },
        ))));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("AgentSession") || msg.contains("TypeId"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_forwarder_succeeds_when_session_loaded_first() {
        let world = World::new();
        let mut set = BundleSet::new(&world);

        let session = Arc::new(RwLock::new(AgentSession {
            server_url: "https://server:3000".into(),
            connected: true,
        }));
        // 先插入 Session 到 World（用 inner value，insert 自动包 Arc<RwLock<>>）
        // 这样 BundleSet::load 检查 deps 时能找到 TypeId::of::<AgentSession>()
        world.insert(AgentSession {
            server_url: "https://server:3000".into(),
            connected: true,
        });

        // Forwarder 依赖满足 → 成功
        set.load(&make_forwarder_bundle(session)).unwrap();
        assert!(world.contains::<ForwarderHandle>());
    }

    #[test]
    fn test_collector_needs_both_session_and_config() {
        let world = World::new();
        let session = Arc::new(RwLock::new(AgentSession {
            server_url: "s".into(),
            connected: true,
        }));
        let config = Arc::new(RwLock::new(DemoConfig::default()));

        // 只插入 Session，不插入 Config → 应失败
        world.insert(AgentSession {
            server_url: "s".into(),
            connected: true,
        });
        let result =
            BundleSet::new(&world).load(&make_collector_bundle(session.clone(), config.clone()));
        assert!(result.is_err());

        // 再插入 Config → 应成功
        world.insert(DemoConfig::default());
        BundleSet::new(&world).load(&make_collector_bundle(session, config)).unwrap();
        assert!(world.contains::<CollectorHandle>());
    }

    // ── 测试：load_all 自动拓扑排序 ──────────────────────────────

    #[test]
    fn test_load_all_topological_reverse_order() {
        let world = World::new();
        let mut set = BundleSet::new(&world);

        let session = Arc::new(RwLock::new(AgentSession {
            server_url: "s".into(),
            connected: true,
        }));
        let config = Arc::new(RwLock::new(DemoConfig::default()));

        let session_bundle = make_session_bundle();
        let config_bundle = make_config_bundle();
        let forwarder = make_forwarder_bundle(session.clone());
        let collector = make_collector_bundle(session, config);

        // 故意倒序传入 — 拓扑排序应纠正
        set.load_all(&[
            &collector as &dyn Bundle,      // 依赖 Session + Config
            &forwarder as &dyn Bundle,      // 依赖 Session
            &config_bundle as &dyn Bundle,  // 无依赖
            &session_bundle as &dyn Bundle, // 无依赖
        ])
        .unwrap();

        // 所有四个类型都应就绪
        assert!(world.contains::<AgentSession>());
        assert!(world.contains::<DemoConfig>());
        assert!(world.contains::<ForwarderHandle>());
        assert!(world.contains::<CollectorHandle>());
    }

    // ── 测试：derive 宏生成的 Bundle trait ───────────────────────

    #[test]
    fn test_derive_bundle_generates_correct_metadata() {
        let bundle = make_session_bundle();
        assert_eq!(bundle.id(), "session");
        assert_eq!(bundle.name(), "Agent Session");
        assert!(!bundle.required());

        let components = bundle.components();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].id, "session");
        assert_eq!(
            components[0].provides,
            std::any::TypeId::of::<AgentSession>()
        );
        assert!(components[0].deps.is_empty());
    }

    #[test]
    fn test_derive_bundle_forwarder_deps_include_session() {
        let session = Arc::new(RwLock::new(AgentSession {
            server_url: "s".into(),
            connected: true,
        }));
        let bundle = make_forwarder_bundle(session);
        let components = bundle.components();
        assert_eq!(
            components[0].deps,
            vec![std::any::TypeId::of::<AgentSession>()]
        );
    }

    #[test]
    fn test_derive_bundle_collector_has_two_deps() {
        let session = Arc::new(RwLock::new(AgentSession {
            server_url: "s".into(),
            connected: true,
        }));
        let config = Arc::new(RwLock::new(DemoConfig::default()));
        let bundle = make_collector_bundle(session, config);
        let components = bundle.components();
        assert_eq!(components[0].deps.len(), 2);
        assert!(components[0].deps.contains(&std::any::TypeId::of::<AgentSession>()));
        assert!(components[0].deps.contains(&std::any::TypeId::of::<DemoConfig>()));
    }

    // ── 测试：PipelineTemplate ────────────────────────────────────

    #[test]
    fn test_demo_pipeline_bundle_template() {
        let bundle = DemoPipelineBundle;
        let templates = bundle.default_pipelines();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "main");
        assert_eq!(templates[0].sources, vec!["periodic_demo"]);
        assert_eq!(
            templates[0].processors,
            vec!["enrich_env", "enrich_collector", "threshold"]
        );
    }
}
