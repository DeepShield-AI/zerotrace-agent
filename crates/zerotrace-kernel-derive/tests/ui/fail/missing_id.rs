//! This file MUST fail to compile — `#[derive(Bundle)]` requires `#[bundle(id = "...")]`.
use zerotrace_kernel_derive::Bundle;
use std::sync::Arc;
use parking_lot::RwLock;

#[derive(Debug)]
struct DbPool;

#[derive(Bundle)]
// #[bundle(id = "...")]  ← MISSING!
struct BadBundle {
    #[component(id = "db", deps = [])]
    db: Arc<RwLock<DbPool>>,
}

fn main() {}
