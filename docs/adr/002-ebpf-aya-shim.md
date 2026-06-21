# ADR-002: eBPF Framework — aya + DeepTrace C Shim

- **Status**: accepted
- **Date**: 2026-05-28
- **Author**: @ioki-smore
- **Deciders**: draft.md v1.1 §15

## Context

DeepFlow uses 15,000+ lines of C eBPF with 5 kernel-version macro branches
(`LINUX_VER_3_10_0` / `5_2_PLUS` / `5_15_PLUS` / `KYLIN` / `KFUNC`), compiled
via libbcc at runtime. This creates high maintenance cost, unreproducible
builds, and a dependency on libbcc/libdwarf/GoReSym native libs.

Alternatives evaluated: (1) `libbpf-rs` + hand-rolled CO-RE, (2) `aya` (pure
Rust BPF loader with native CO-RE), (3) stay with libbcc.

## Decision

**aya (kernel-side Rust) + DeepTrace-style C shim** for CO-RE struct field
access. The shim is ~100 lines of C declaring `SHIM(struct, member)` macros
that expand to `BPF_CORE_READ` wrappers. `clang -target bpf -g` compiles the
shim with BTF; `bindgen` generates Rust FFI functions that aya-ebpf Rust code
calls directly.

## Consequences

### Positive
- Eliminates libbcc/libdwarf from the runtime dependency chain.
- CO-RE relocation handled by aya's built-in BTF resolver + btfhub-archive
  fallback for older kernels.
- Kernel-side code is in Rust (memory safety, no `static mut`).

### Negative
- aya's CO-RE struct field access is less ergonomic than C `BPF_CORE_READ`
  (mitigated by the shim).
- Not all DeepFlow probes migrate at once — `perf_profiler`/`go_http2`/
  `files_rw` continue on the old C path during transition.

## Alternatives Considered

| Alternative | Why rejected |
|---|---|
| libbpf-rs + hand-rolled CO-RE | Same maintenance burden as C BPF — just in Rust syntax |
| Stay with libbcc | Conflicts with SaaS agent goals (no runtime compiler dependency) |
| aya without shim | Requires manually writing CO-RE relocation logic in Rust, which is error-prone |
