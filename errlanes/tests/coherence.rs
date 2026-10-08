//! Compiles `upstream`/`downstream` (a dev-dependency of this crate) so the
//! six `?` lane inclusion paths and the narrowed match from Appendix A
//! of the error-handling research doc are checked on every `cargo test`, and
//! pins that downstream crates cannot implement a generic conversion between
//! two foreign `Fail` types.

#[test]
fn downstream_exercises_every_lane_inclusion_path() {
    downstream::smoke();
}

#[test]
fn from_fail_a_for_fail_b_downstream_is_e0117() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/coherence/e0117.rs");
}

/// Appendix B of the foreign-error interop design doc, pinned here rather
/// than in `errlanes-derive`'s trybuild suite because these cases exercise
/// the core traits and blanket impls directly, independent of any derive.
#[test]
fn classify_appendix_b_negative_cases() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/coherence/classify_u1_rejection_into_fault.rs");
    t.compile_fail("tests/coherence/classify_u2_mixed_wrapper_into_fault.rs");
    t.compile_fail("tests/coherence/classify_u4_rejection_and_classify_conflict.rs");
    t.compile_fail("tests/coherence/classify_n_mixed_wrapper_into_bare_fatal.rs");
}

/// Carrier misuse, each pinned to the reason it must fail (see the comment
/// at the top of every case).
#[test]
fn carrier_negative_cases() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/coherence/carrier_fail/*.rs");
}
