#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.pass("tests/trybuild/rejection_pass.rs");
    t.pass("tests/trybuild/failure_pass.rs");
    t.pass("tests/trybuild/classify_pass.rs");
    t.compile_fail("tests/trybuild/classify_missing_lane.rs");
    t.compile_fail("tests/trybuild/rejection_constraint_without_repo.rs");
    t.compile_fail("tests/trybuild/failure_from_not_failure.rs");
}
