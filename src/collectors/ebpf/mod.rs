// eBPF collector — Source implementation for syscall/tracepoint/uprobe events.
pub mod legacy;
pub mod btf_resolver;
pub mod loader;
pub mod socket_source;
pub mod tls;
