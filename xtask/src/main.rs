//! xtask — workspace build helpers.
//!
//! Run via `cargo xtask <subcommand>`. The alias is defined in `.cargo/config.toml`.
//!
//! Subcommands are intentionally thin wrappers; for one-off scripts add a new
//! variant rather than reaching for a shell script.

use anyhow::Result;
use clap::{Parser, Subcommand};
use xshell::{cmd, Shell};

#[derive(Parser)]
#[command(name = "xtask", about = "ZeroTrace workspace tooling")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run fmt + clippy + nextest + cargo-deny in order. CI entrypoint.
    Check {
        /// Skip cargo-deny (e.g. when offline).
        #[arg(long)]
        no_deny: bool,
    },
    /// Run cargo fmt across the workspace.
    Fmt {
        /// Check only; do not modify files.
        #[arg(long)]
        check: bool,
    },
    /// Run clippy with workspace lints.
    Clippy,
    /// Run tests via cargo-nextest if available, else cargo test.
    Test,
    /// Run cargo-deny check.
    Deny,
    /// Regenerate workspace-hack via cargo-hakari.
    HakariGen,
    /// Generate JSON schemas for agent config (output: docs/schemas/).
    GenSchema,
    /// Bundle BTF blobs for supported distros (output: resources/btf/).
    /// Stubbed until M3 W9.
    BtfBundle,
    /// Build and package a release artifact set.
    /// Stubbed until M4 W13.
    Release {
        #[arg(long, default_value = "x86_64-unknown-linux-gnu")]
        target: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sh = Shell::new()?;
    match cli.cmd {
        Cmd::Check { no_deny } => check(&sh, no_deny),
        Cmd::Fmt { check } => fmt(&sh, check),
        Cmd::Clippy => clippy(&sh),
        Cmd::Test => test(&sh),
        Cmd::Deny => deny(&sh),
        Cmd::HakariGen => hakari_gen(&sh),
        Cmd::GenSchema => gen_schema(&sh),
        Cmd::BtfBundle => btf_bundle(&sh),
        Cmd::Release { target } => release(&sh, &target),
    }
}

fn fmt(sh: &Shell, check: bool) -> Result<()> {
    if check {
        cmd!(sh, "cargo fmt --all -- --check").run()?;
    } else {
        cmd!(sh, "cargo fmt --all").run()?;
    }
    Ok(())
}

fn clippy(sh: &Shell) -> Result<()> {
    cmd!(
        sh,
        "cargo clippy --workspace --all-targets --no-default-features -- -D warnings"
    )
    .run()?;
    Ok(())
}

fn test(sh: &Shell) -> Result<()> {
    if cmd!(sh, "cargo nextest --version").quiet().run().is_ok() {
        cmd!(sh, "cargo nextest run --workspace --no-default-features").run()?;
    } else {
        cmd!(sh, "cargo test --workspace --no-default-features").run()?;
    }
    Ok(())
}

fn deny(sh: &Shell) -> Result<()> {
    cmd!(sh, "cargo deny check").run()?;
    Ok(())
}

fn check(sh: &Shell, no_deny: bool) -> Result<()> {
    fmt(sh, true)?;
    clippy(sh)?;
    test(sh)?;
    if !no_deny {
        deny(sh)?;
    }
    Ok(())
}

fn hakari_gen(sh: &Shell) -> Result<()> {
    cmd!(sh, "cargo hakari generate").run()?;
    cmd!(sh, "cargo hakari manage-deps --yes").run()?;
    cmd!(sh, "cargo hakari verify").run()?;
    Ok(())
}

fn gen_schema(_sh: &Shell) -> Result<()> {
    println!("TODO(M0 W2): emit JSON schema from zerotrace-config to docs/schemas/");
    Ok(())
}

fn btf_bundle(_sh: &Shell) -> Result<()> {
    println!("TODO(M3 W9): fetch btfhub-archive subset into resources/btf/");
    Ok(())
}

fn release(_sh: &Shell, target: &str) -> Result<()> {
    println!("TODO(M4 W13): build release for target={target}, package into dist/");
    Ok(())
}
