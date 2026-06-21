//! ZeroTrace DI Framework Demo — 演示完整的 DI 框架能力。
//!
//! # 模块结构
//!
//! - `signals` — 自定义信号类型 (`#[derive(SignalType)]`)
//! - `config` — YAML 配置类型 (serde)
//! - `collectors` — Source 实现 (Real/Noop/Mock)
//! - `processors` — Processor 实现 (Real/Noop/Spy)
//! - `reporters` — Reporter 实现 (Real/Noop)
//! - `bundles` — Bundle 定义 (`#[derive(Bundle)]` + 手动 impl)
//!
//! # 测试层级
//!
//! | 层级 | 位置 | 测试什么 |
//! |------|------|---------|
//! | L1 组件单元测试 | 各模块 `#[cfg(test)]` | 纯 trait 方法 |
//! | L2 Bundle 依赖链 | `bundles::tests` | TypeId 依赖 + 拓扑排序 |
//! | L3 Pipeline 集成 | `tests/integration.rs` | 完整 Source→Reporter 链路 |
//! | L4 E2E | `main.rs` | YAML 驱动全链路 |

pub mod blueprint_ext;
pub mod bundles;
pub mod collectors;
pub mod config;
pub mod config_resolver;
pub mod pipeline_isolation;
pub mod processors;
pub mod reporters;
pub mod signals;
