// The motivating shape from the `errlanes-error-format-arguments` handoff:
// a named placeholder (`{id}`), a bare auto-indexed `{}`, and a trailing
// format argument that reaches *through* a composite field (`failure.error`)
// to a member that implements `Display`, while the field's own type
// (`DecodeFailure`) does not. Before trailing arguments were supported, this
// failed to parse at all (`unexpected token`, caret on the comma); working
// around it by naming `{failure}` moved the `Display` bound onto
// `DecodeFailure` itself, which has none.
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

fn main() {
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
