# ADR-004: Crate Organization — Fat Binary + Thin Tool Crates

- **Status**: accepted
- **Date**: 2026-05-28
- **Author**: @ioki-smore
- **Deciders**: draft.md v1 §7

## Context

We need to organize 30+ components (sources, processors, reporters, bundles)
across a Rust workspace. Two established patterns in the ecosystem:

1. **Many crates** (Bevy, rustc): each component is a separate crate with its
   own Cargo.toml.  Compile times improve (incremental), but crate boundaries
   add boilerplate and mental overhead for contributors.
2. **Fat binary + thin libraries** (Vector, cargo, wasmtime): business logic
   lives in `src/` as modules; only shared infrastructure (kernel, config,
   platform) gets its own crate.

## Decision

**Vector-style fat binary**.  Business components (sources/processors/reporters/
bundles) are `src/` modules.  Only these get their own crate:

- Infrastructure that is stable, independently testable, and reusable (kernel,
  core, runtime, config, platform).
- Crates with incompatible target triples (ebpf-kernel: no_std, bpf target).
- Third-party plugin ABI (need C ABI stability guarantees).

workspace members: 8 tool crates + 1 ebpf-kernel + 5 legacy + 17 plugins +
xtask + workspace-hack + agent-ctl ≈ 33 total.

## Consequences

### Positive
- New features = new `.rs` file, not a new crate → low barrier to entry.
- `cargo check` on the fat bin covers all business code at once.

### Negative
- Full build time grows linearly with `src/` size (mitigated by workspace-hack
  and `cargo build --timings` monitoring in continuous task P2).
- Module visibility (`pub(crate)`) harder to enforce across a 100+ module tree.

## Alternatives Considered

| Alternative | Why rejected |
|---|---|
| One crate per component | ~50 Cargo.toml files; contributors spend more time on crate boundaries than on logic |
| Single monolithic crate (no workspace) | Cannot isolate ebpf-kernel (no_std) or enforce dependency direction |
