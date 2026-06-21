// Legacy DeepFlow modules — kept for compatibility during the M0–M4 migration.
// All modules here will be deleted or migrated by the end of M4.
//
// Do NOT add new code here.  New features go under:
//   src/collectors/  src/processors/  src/reporters/  src/bundles/

pub mod collector;
pub mod dispatcher;
#[cfg(all(unix, feature = "libtrace"))]
pub mod ebpf_dispatcher;
pub mod flow_generator;
pub mod handler;
pub mod integration_collector;
pub mod metric;
pub mod monitor;
pub mod platform;
pub mod plugin;
pub mod policy;
pub mod rpc;
pub mod sender;
pub mod trident;
