# xtask

Workspace tooling for ZeroTrace. Subcommands wrap common cargo + cargo-* invocations so CI and local dev share one entry point.

## Usage

All commands run from the repo root and use the alias defined in `.cargo/config.toml`:

```bash
cargo xtask <subcommand>
```

| Subcommand | Effect |
|---|---|
| `check` | fmt + clippy + nextest + cargo-deny (CI entrypoint) |
| `fmt [--check]` | `cargo fmt --all` |
| `clippy` | `cargo clippy --workspace --all-targets --no-default-features -- -D warnings` |
| `test` | `cargo nextest run` if installed, else `cargo test` |
| `deny` | `cargo deny check` |
| `hakari-gen` | Regenerate `workspace-hack` (requires `cargo install cargo-hakari`) |
| `gen-schema` | (stub, M0 W2) JSON schema for agent config |
| `btf-bundle` | (stub, M3 W9) bundle btfhub-archive subset |
| `release --target <T>` | (stub, M4 W13) build + package release artifact |

## Tools to install once

```bash
cargo install cargo-deny cargo-hakari cargo-nextest cargo-machete typos-cli
rustup component add rustfmt clippy rust-src
```

## Local-host build note

`.cargo/config.toml` pins the linker to `/opt/rh/devtoolset-{8,11}/...` because that's the path inside the `ghcr.io/zerotraceio/rust-build` CI image. On a normal Linux host without that toolset, builds fail with `exit status: 127` (file not found).

Two workarounds:

1. **Use the CI Docker image** (recommended for full builds that touch eBPF / native deps).
2. **Override the linker via env** for local Rust-only crates such as `xtask`:

   ```bash
   CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=/usr/bin/cc cargo xtask check
   ```

   Or pin permanently in `~/.cargo/config.toml`:

   ```toml
   [target.x86_64-unknown-linux-gnu]
   linker = "/usr/bin/cc"
   ```

   User-level config overrides repo-level config for fields like `linker`.

## Adding a new subcommand

Edit `xtask/src/main.rs`:

1. Add a variant to `enum Cmd`.
2. Add a handler function.
3. Wire it in `main()`'s `match`.

Prefer Rust over bash for any logic past 10 lines — `xshell` makes shell-style commands ergonomic without losing type safety.
