#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/v2/*.rs");
    t.pass("tests/trybuild/rejection_pass.rs");
    t.pass("tests/trybuild/rejection_multi_lift.rs");
    t.pass("tests/trybuild/failure_pass.rs");
    t.compile_fail("tests/trybuild/failure_from_not_failure.rs");
    t.compile_fail("tests/trybuild/fault_is_not_failure.rs");
}
