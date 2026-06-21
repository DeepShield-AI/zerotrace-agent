//! This file MUST fail to compile — `#[derive(Bundle)]` requires named fields.
use zerotrace_kernel_derive::Bundle;

#[derive(Bundle)]
#[bundle(id = "bad", name = "Bad")]
struct BadBundle;  // no fields

fn main() {}
