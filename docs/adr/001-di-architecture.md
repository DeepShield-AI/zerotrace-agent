# ADR-001: DI Architecture — Bevy World + Datadog Component Model

- **Status**: accepted
- **Date**: 2026-05-28
- **Author**: @ioki-smore
- **Deciders**: draft.md v1.1

## Context

ZeroTrace is a fork of DeepFlow with a monolithic `trident.rs` (4123 lines)
and `config/handler.rs` (6075 lines). We need to (1) make components
independently testable, (2) allow new AI/security modules to be added without
touching core code, and (3) enable runtime configuration via YAML + SIGHUP.

Two mature DI patterns in Rust were evaluated: **Bevy ECS** (World, SystemParam)
and **Datadog Agent comp/** (trait-based components, fx.Lifecycle, Bundle).

## Decision

**Hybrid**: Bevy's `World` + `SystemParam` for the DI container (Rust-native,
no reflection overhead), combined with Datadog's **interface/implementation
separation** (trait → real/noop/mock) and **Bundle grouping** for business
domain boundaries.

| Layer | What | Origin |
|---|---|---|
| L1 Kernel | World, SystemParam, LifecycleRegistry, ConfigBus | Bevy + Datadog hybrid |
| L2 Component | trait Source/Processor/Reporter + real/noop/mock | Datadog comp/ |
| L3 Bundle | Component groups with config schema | Datadog comp/ |
| L4 Pipeline | YAML DSL → Source → mpsc → Processor → mpsc → Reporter | Original |

## Consequences

### Positive
- Components are independently testable (mock implementations are mandatory).
- New AI/security modules add `Signal::Custom(Arc<dyn ErasedSignal>)` without
  modifying the core enum.
- Old code (trident.rs, dispatcher) can coexist with new Components via the
  "parasitic" World strategy (§10 of draft.md).

### Negative
- `World` is TypeId-keyed → only one instance per type (need newtypes for
  multiple instances).
- `#[async_trait]` overhead on every lifecycle/producer call (acceptable for
  startup/shutdown; may need optimization for hot-path `process()`).

## Alternatives Considered

| Alternative | Why rejected |
|---|---|
| Pure Bevy ECS (System, Schedule, App) | Agent has no tick loop — Scheduler abstraction adds complexity without benefit |
| Pure Datadog fx (Go reflection) | Rust has no reflection — need explicit TypeId-based World |
| Tower/Tower-layer stack | Excellent for request/response but awkward for streaming Source→Reporter pipelines |
