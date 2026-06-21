// Compile-fail / pass tests for the derive macros.
// Uses trybuild to verify that correct code compiles and incorrect code
// produces the expected error messages.

#[test]
fn derive_bundle_ui_pass() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_basic.rs");
}

#[test]
fn derive_bundle_ui_fail_missing_id() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/fail/missing_id.rs");
}

#[test]
fn derive_bundle_ui_fail_no_fields() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/fail/no_fields.rs");
}

#[test]
fn derive_bundle_ui_fail_not_a_struct() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/fail/not_a_struct.rs");
}
