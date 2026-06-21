# ZeroTrace Developer Guide

> For **contributors** getting started, read [CONTRIBUTING.md](CONTRIBUTING.md) first.
> This document is the **internal reference** for the codebase architecture,
> patterns, and workflows.

## 1. Architecture overview

ZeroTrace follows a **four-layer architecture**:

```
L4  Pipeline   YAML DSL → Source → mpsc → Processor → mpsc → Reporter
L3  Bundle     Component groups (host-metric, microservice, cloud-platform…)
L2  Component  trait Source / Processor / Reporter + real / noop / mock
L1  Kernel     World (DI) / SystemParam / Lifecycle / ConfigBus / Bundle trait
```

**Key design decisions** are documented as ADRs in [docs/adr/](adr/). Read them
before making architectural changes.

### Dependency direction (STRICTLY ONE-WAY)

```
                      core
                       ▲
                     kernel
                       ▲
       ┌──────┬─────────┼────────┬────────┐
       │      │         │        │        │
   runtime  config  platform  forwarder  debug
       ▲      ▲         ▲        ▲        ▲
       └──────┴────┬────┴────────┴────────┘
                   │
            main bin (zerotrace-agent)
```

- `core`: zero dependencies (only std + serde + thiserror).
- `kernel`: only depends on `core` + tokio + parking_lot + async-trait.
- `ebpf-kernel`: no_std, `target = bpfel-unknown-none`, not linked into main bin.
- Main bin: top-level consumer, never a dependency of any crate.

## 2. Crate map

| Crate | Location | Purpose | Key files |
|---|---|---|---|
| `zerotrace-core` | `crates/zerotrace-core/` | Signal types, Error | `signal.rs`, `error.rs` |
| `zerotrace-kernel` | `crates/zerotrace-kernel/` | DI container, lifecycle, bundle | `world.rs`, `param.rs`, `lifecycle.rs`, `config_bus.rs`, `bundle.rs`, `metrics.rs` |
| `zerotrace-runtime` | `crates/zerotrace-runtime/` | Pipeline executor, bundle loader | `pipeline.rs`, `loader.rs` |
| `zerotrace-config` | `crates/zerotrace-config/` | YAML parsing, hot-reload | `lib.rs`, `watcher.rs` |
| `zerotrace-forwarder` | `crates/zerotrace-forwarder/` | HTTP forwarder to server | `control.rs`, `data.rs`, `auth.rs` |
| `zerotrace-platform` | `crates/zerotrace-platform/` | Cloud metadata (IMDS, K8s) | (stub) |
| `zerotrace-debug` | `crates/zerotrace-debug/` | Debug socket, ctl | (stub) |
| `zerotrace-plugin-abi` | `crates/zerotrace-plugin-abi/` | Stable C ABI for .so | (stub) |
| `zerotrace-agent` | `src/` | Main binary and all components | `main.rs`, `collectors/`, `processors/`, `reporters/`, `bundles/` |

### Legacy crates (maintained, gradually migrated)

| Crate | Purpose |
|---|---|
| `public` | Shared types and utilities (DeepFlow heritage) |
| `public-derive` | Derive macros for `public` |
| `public-derive-internals` | Internal derive support |
| `trace-utils` | Trace-level utilities |
| `enterprise-utils` | Enterprise features |
| `plugins/*` | 17 plugin crates (ebpf, protocol parsers, packet processors) |

## 3. Patterns

### 3.1 Adding a new Component

Every component follows the **Datadog comp/ pattern**: trait (interface) +
three implementations.

```
src/collectors/proc/cpu/
├── mod.rs          # trait CpuCollector + re-exports
├── real.rs         # impl from /proc/stat
├── noop.rs         # empty impl (used when disabled)
└── mock.rs         # #[cfg(any(test, feature = "test-utils"))] controlled behavior
```

Steps:

1. Define the trait in `mod.rs`:

```rust
#[async_trait]
pub trait CpuCollector: Source<Output = MetricBatch> + Lifecycle {
    fn cpu_count(&self) -> usize;
}
```

2. Implement `real.rs` with constructor injection:

```rust
pub struct RealCpuCollector { /* fields */ }

impl RealCpuCollector {
    pub fn new(cfg: Cfg<CpuCollectorConfig>, registry: &mut LifecycleRegistry) -> Arc<Self> {
        let me = Arc::new(Self { /* … */ });
        registry.register(me.clone()); // self-register lifecycle
        me
    }
}
```

3. Add to a Bundle:

```rust
// src/bundles/host_metric.rs
impl Bundle for HostMetricBundle {
    fn components(&self) -> Vec<ComponentDescriptor> {
        vec![ComponentDescriptor {
            id: "source.proc.cpu",
            provides: TypeId::of::<dyn CpuCollector>(),
            deps: vec![TypeId::of::<CpuCollectorConfig>()],
            factory: Box::new(|world, registry| {
                let cfg = Cfg::fetch(world)?;
                Ok(RealCpuCollector::new(cfg, registry))
            }),
        }]
    }
}
```

4. Test with the mock:

```rust
#[cfg(test)]
mod tests {
    use super::mock::MockCpuCollector;
    // inject controlled values, verify behavior
}
```

### 3.2 Dependency Injection via World

```rust
// Components declare dependencies in their constructor signature
pub fn new(cfg: Cfg<MyConfig>, metrics: Res<KernelMetrics>) -> Arc<Self> { … }

// At build time, BundleLoader calls SystemParam::fetch(world) for each arg.
// Res<T>   → Arc<T> from World (shared read-only)
// Cfg<T>   → Arc<T> from World (hot-reload snapshot)
// Sender<T> / Recv<T> → injected by PipelineExecutor (not from World)
```

### 3.3 Async trait dispatch

- `#[async_trait]` is used for `Lifecycle`, `Source`, `Processor`, `Reporter`.
- **Startup/shutdown paths** (Lifecycle hooks): async_trait overhead is acceptable.
- **Hot paths** (`Source::run`, `Processor::process`): if profiling shows
  >5% CPU spent in `Pin<Box<dyn Future>>` allocation, replace with enum
  dispatch or hand-rolled `poll_*` methods.

## 4. Testing strategy

| Layer | Tool | What it covers | When |
|---|---|---|---|
| Unit | `cargo test` | Individual functions and methods | Every commit |
| Integration | `cargo test` with `src/` fixtures | Cross-component pipelines | Before PR |
| Fuzz | `cargo +nightly fuzz` | Protocol parsers (OTel, Prometheus, …) | Weekly + before release |
| MIRI | `cargo +nightly miri test` | Unsafe code UB detection | When unsafe code changes |
| Benchmark | `cargo bench` | Hot-path latency regression | PR (CI `bench-cmp`) |
| Loom | `cargo test --features loom` | Concurrency correctness | When sync primitives change |
| Soak | Manual (W15) | Memory leaks, RSS stability | Before release |

### Running MIRI

```bash
# Nightly toolchain required
rustup +nightly component add miri
cargo +nightly miri test -p zerotrace-kernel -- world
```

### Running fuzz targets

```bash
cargo +nightly fuzz run otel_receiver -- -max_total_time=60
cargo +nightly fuzz run prometheus_receiver -- -max_total_time=60
```

## 5. Unsafe code

Policy: **Firecracker standard** (see [ADR-001](adr/001-di-architecture.md)).

- `#![deny(unsafe_code)]` at crate root (default deny).
- `#![allow(unsafe_code)]` only in modules that genuinely need it (currently:
  `zerotrace-kernel/src/world.rs`).
- Every `unsafe` block must have a `// SAFETY:` comment.
- Each crate with unsafe code has a `SAFETY.md` inventory.
- New unsafe code must pass `cargo xtask miri` before merge.

When adding unsafe code to a new module, update:

1. The module header: add `#![allow(unsafe_code)]`.
2. The crate's `SAFETY.md`: add a row for each unsafe block.
3. The CI `miri` job: if adding a new target.

## 6. Common workflows

### Before pushing a PR

```bash
cargo xtask check          # fmt + clippy + test + deny
cargo xtask typos          # spell-check
cargo hakari generate --diff  # update workspace-hack if needed
```

### Adding a dependency

1. Add it to the appropriate `Cargo.toml` (workspace `[workspace.dependencies]`
   if shared, crate-local if single-use).
2. Run `cargo hakari generate` to update `workspace-hack/`.
3. Run `cargo xtask audit` to check for advisories.
4. Run `cargo machete` to verify no unused deps.

### Debugging a failing test

```bash
# Single test with backtrace
RUST_BACKTRACE=1 cargo test -p zerotrace-kernel -- world::tests::test_insert_and_get

# With logging
RUST_LOG=debug cargo test -p zerotrace-kernel

# Under MIRI (if it involves unsafe)
cargo +nightly miri test -p zerotrace-kernel -- world::tests::test_insert_and_get
```

### Hot-reload agent config

```bash
# Edit /etc/zerotrace-agent.yaml
# Send SIGHUP
kill -SIGHUP $(pidof zerotrace-agent)

# Check config dispatch latency
# (via debug socket or kernel metrics snapshot)
```

## 7. xtask reference

`xtask` is the project's build automation entry point. All common workflows
are a single `cargo xtask <subcommand>` away.

| Command | Description |
|---|---|
| `cargo xtask check` | fmt + clippy + test + deny (CI entry point) |
| `cargo xtask fmt [--check]` | Format or check formatting |
| `cargo xtask clippy` | Run clippy with `-D warnings` |
| `cargo xtask test` | Run tests (nextest if available) |
| `cargo xtask deny` | Run cargo-deny |
| `cargo xtask audit` | cargo-deny advisories + cargo-audit |
| `cargo xtask typos` | Spell-check with typos-cli |
| `cargo xtask miri` | Run MIRI on unsafe modules |
| `cargo xtask bench-cmp` | Compare benchmarks against `main` branch |
| `cargo xtask fuzz <target>` | Run a specific fuzz target |
| `cargo xtask setup-dev` | Install all required tools |
| `cargo xtask hakari-gen` | Regenerate workspace-hack |
| `cargo xtask gen-schema` | Generate JSON schemas (M0 W2) |
| `cargo xtask btf-bundle` | Bundle BTF blobs (M3 W9) |
| `cargo xtask release` | Build release artifacts (M4 W13) |

## 8. Release checklist

Before tagging a release:

- [ ] `cargo xtask check` — green
- [ ] `cargo xtask audit` — 0 critical advisories
- [ ] `cargo xtask typos` — no spelling errors
- [ ] `cargo xtask miri` — 0 UB
- [ ] `cargo bench --workspace` — no regression >5% vs previous release
- [ ] Soak test — 7 days no crash, RSS < 300MB
- [ ] Threat model review — no open HIGH findings
- [ ] `git tag vX.Y.Z` and push
