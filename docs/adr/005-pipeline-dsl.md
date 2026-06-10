# ADR-005: Pipeline Configuration — YAML DSL

- **Status**: accepted
- **Date**: 2026-05-28
- **Author**: @ioki-smore
- **Deciders**: draft.md v1 §6

## Context

Pipelines connect Sources → Processors → Reporters. Users need to declare which
components to use, in what order, and with what configuration. The format must
be: (1) human-readable, (2) support SIGHUP hot-reload, (3) support component
refs with inline config overrides, (4) familiar to DevOps/SRE teams.

## Decision

**YAML** (`/etc/zerotrace-agent.yaml`) with the following structure:

```yaml
bundles: [core, host-metric, microservice]   # L3: what to load
pipelines:                                    # L4: how to connect
  host_metrics:
    sources: [{ref: cpu}, {ref: memory}]
    processors: [{ref: tagging}]
    reporters: [{ref: http_to_server, config: {batch_size: 500}}]
```

Components are referenced by their `ref` name (registered in World by their
Bundle). PipelineExecutor validates type compatibility at build time.

## Consequences

### Positive
- SREs already know YAML (Kubernetes, Ansible, Docker Compose).
- `serde_yaml` + `serde_json` provide schema validation out of the box.
- Hot-reload via SIGHUP → ConfigWatcher → ConfigBus → subscriber components.

### Negative
- YAML's implicit type coercion can cause subtle config bugs (e.g., `enabled: no`
  vs `enabled: false`).
- No IDE autocomplete (mitigated by providing JSON Schema generated from
  `schemars` derives in a future task).

## Alternatives Considered

| Alternative | Why rejected |
|---|---|
| TOML | No built-in anchor/alias; less familiar to SRE audience |
| HCL (Terraform syntax) | Requires external parser; overkill for agent config |
| JSON (only) | No comments allowed — painful for human-maintained config |
