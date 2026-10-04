#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/v2/*.rs");
    t.pass("tests/trybuild/rejection_pass.rs");
    t.pass("tests/trybuild/rejection_multi_lift.rs");
    t.pass("tests/trybuild/failure_pass.rs");
    t.pass("tests/trybuild/error_format_args_pass.rs");
    t.compile_fail("tests/trybuild/error_format_args_unknown_field.rs");
    t.compile_fail("tests/trybuild/error_format_args_arbitrary_expr.rs");
    t.compile_fail("tests/trybuild/error_format_args_bare_auto_index_still_rejected.rs");
    t.compile_fail("tests/trybuild/failure_from_not_failure.rs");
    t.compile_fail("tests/trybuild/fault_is_not_failure.rs");
    t.pass("tests/trybuild/instrument_async_with_fields.rs");
    t.pass("tests/trybuild/instrument_async_without_fields.rs");
    t.pass("tests/trybuild/instrument_sync.rs");
    t.pass("tests/trybuild/instrument_fields_declares_error.rs");
    t.pass("tests/trybuild/instrument_trailing_comma.rs");
    t.compile_fail("tests/trybuild/instrument_not_laned.rs");
}

// A direct (non-trybuild) runtime check that trailing `#[error(..)]`
// arguments render byte-identically to what `thiserror` produced for the
// same shape — see the `errlanes-error-format-arguments` handoff. The
// trybuild pass case above already asserts this once as a derive-macro
// smoke test; this duplicates the assertion as an ordinary `cargo test`,
// outside a trybuild subprocess, so it shows up directly in normal test
// output and under `cargo nextest`.
#[derive(Debug)]
struct DecodeFailure {
    error: String,
}

#[derive(Debug, errlanes::Classify)]
#[classify(fatal(CorruptState))]
#[error("undecodable event {id} at sequence {sequence}: {}", failure.error)]
struct UndecodableEvent {
    id: i32,
    sequence: i32,
    failure: DecodeFailure,
}

#[test]
fn error_format_args_render_composite_field_member() {
    let event = UndecodableEvent {
        id: 7,
        sequence: 42,
        failure: DecodeFailure {
            error: "missing field `foo`".to_string(),
        },
    };
    assert_eq!(
        event.to_string(),
        "undecodable event 7 at sequence 42: missing field `foo`"
    );
}
