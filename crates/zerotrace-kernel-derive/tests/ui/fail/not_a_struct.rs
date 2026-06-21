//! This file MUST fail to compile — `#[derive(Bundle)]` only works on structs.
use zerotrace_kernel_derive::Bundle;

#[derive(Bundle)]
#[bundle(id = "bad")]
enum BadEnum {
    Variant,
}

fn main() {}
