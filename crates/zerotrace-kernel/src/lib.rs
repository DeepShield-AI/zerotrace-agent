// zerotrace-kernel: Bevy-inspired DI container.
// Uses Arc<RwLock<T>> internally — safe, simple, zero-unsafe in public API.
pub mod app;
pub mod bundle;
pub mod config_bus;
pub mod error;
pub mod event;
pub mod lifecycle;
pub mod metrics;
pub mod param;
pub mod system;
pub mod world;

// Re-exports for convenience
pub use system::{AsyncFunctionSystem, AsyncSystem, IntoAsyncSystem};
pub use zerotrace_kernel_derive::Bundle;
