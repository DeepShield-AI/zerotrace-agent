# Contributing to ZeroTrace

## Before you start

- Read [ARCHITECTURE.md](ARCHITECTURE.md) (or the current design in [draft.md](../draft.md)) for the high-level architecture.
- Read [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md) for a walk-through of the codebase and common workflows.
- Read [docs/adr/](adr/) for the rationale behind key design decisions.
- Chat with the team in the WeChat group (QR in [wechat-group-keeper.png](wechat-group-keeper.png)).

## Development workflow

We follow the standard [development workflow](development_workflow.md):

1. **Branch** from `dev`: `git checkout -b yourname/feature-name`
2. **Commit** in small, logical chunks using [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`
3. **Push** your branch and open a PR against `dev`.
4. **Review** — at least one team member must approve before merge.
5. **Merge** — squash-and-merge into `dev`.

## One-command dev setup

```bash
# 1. Install Rust toolchain + all required tools
cargo xtask setup-dev

# 2. Build the project (no eBPF for a quick check)
cargo xtask check

# 3. Run tests
cargo xtask test

# 4. Open in your IDE — recommended: VS Code with rust-analyzer
code .
```

## Quality gates that run on every PR

| Gate | Command | What it checks |
|---|---|---|
| Format | `cargo xtask fmt --check` | rustfmt |
| Lint | `cargo xtask clippy` | clippy with `-D warnings` |
| Test | `cargo xtask test` | nextest (falls back to `cargo test`) |
| Audit | `cargo xtask audit` | cargo-deny advisories + cargo-audit |
| Bench | CI `bench-cmp` job | `critcmp pr main` → fail if >5% slower |
| Typo | `cargo xtask typos` | spell-check comments and docs |

## What to work on

- Check the [todo.md](../todo.md) task board for planned work.
- Issues tagged `good first issue` are suitable for new contributors.
- If you're unsure, ask in the WeChat group or open a discussion.

## Code standards

### Unsafe code

This project follows the **Firecracker standard** for unsafe code:

1. `#![deny(unsafe_code)]` is the default for every crate.
2. Modules that genuinely need `unsafe` must have `#![allow(unsafe_code)]` at the module level.
3. Every `unsafe` block must be preceded by a `// SAFETY:` comment explaining:
   - The invariant that makes the block sound.
   - Why the invariant holds at this call site.
4. Each module with unsafe code must have an entry in the crate's `SAFETY.md`.
5. New unsafe code must pass MIRI: `cargo xtask miri`.

### Error handling

- **Library crates** (core, kernel, runtime): use `thiserror` for structured errors. No `String` in error types — use `Box<dyn Error + Send + Sync>` for catch-all variants.
- **Application code** (main bin): use `anyhow::Result` for fallible functions, `anyhow::Context` for adding context.

### Testing

- Every new Component must have **real**, **noop**, and **mock** implementations — unit-test the real impl with mocked dependencies.
- Protocol parsers that accept external input must have a `cargo fuzz` target.
- Hot-path code (`World::get`, pipeline channel ops) must have a criterion benchmark and a MIRI run.

## Project structure quick reference

```
crates/
  zerotrace-core/        ← Signal types, Error (zero dependencies)
  zerotrace-kernel/      ← World, SystemParam, Lifecycle, ConfigBus, Bundle
  zerotrace-runtime/     ← PipelineExecutor, BundleLoader
  zerotrace-config/      ← YAML parsing, schema, hot-reload
  zerotrace-forwarder/   ← HTTP forwarder to server
  zerotrace-platform/    ← IMDS, K8s, libvirt metadata
  zerotrace-debug/       ← Debug socket, ctl protocol
  zerotrace-plugin-abi/  ← Stable C ABI for .so plugins
src/
  collectors/            ← All Source implementations
  processors/            ← All Processor implementations
  reporters/             ← All Reporter implementations
  bundles/               ← Bundle registrations
  extensions/            ← Extension examples
```

Dependency direction is **strictly one-way**: `core → kernel → runtime/config → main bin`.
