# ADR-003: Agent→Server Communication — HTTP Short-Lived Connections

- **Status**: accepted
- **Date**: 2026-05-28
- **Author**: @ioki-smore
- **Deciders**: draft.md v1 §8

## Context

DeepFlow uses a **bidirectional gRPC stream** between agent and server. This
requires persistent connections, complex reconnection logic, and makes the
server responsible for pushing config and remote-exec commands to agents.

For ZeroTrace's SaaS model: (1) the server is operated by us, not the customer;
(2) agents may be behind NAT/firewalls; (3) API key authentication is required.

## Decision

**HTTP short-lived connections with API key auth**. Agent polls the server
periodically (heartbeat every 5s, config poll every 30s). Data plane uses
`POST /api/v1/data/ingest` with wire-format frames (zstd-compressed protobuf),
preserving byte-level compatibility with the existing ingester pipeline.

Data route: **Option 1.A** — wire frames → server `/data/ingest` → ingester →
`flow_log.*`, same format as the existing TCP path.

## Consequences

### Positive
- No persistent connection state — simpler server, easier load balancing.
- API key auth (sha256 hash comparison) at the `/api/v1` group level.
- Agent offline mode: debug data stays on local disk; no stream to reconnect.

### Negative
- 5s heartbeat = up to 5s latency for config changes (acceptable for infra).
- No server push — remote-exec becomes agent-poll (already implemented as
  `/api/v1/agent/remote_exec/{poll,result}`).

## Alternatives Considered

| Alternative | Why rejected |
|---|---|
| Keep gRPC stream | Requires bidirectional state; conflicts with SaaS model |
| WebSocket | Same complexity as gRPC stream; no standard protobuf framing |
| Data per-endpoint (`/data/metric`, `/data/trace`, ...) | Simpler DTOs but incompatible with existing ingester wire format |
