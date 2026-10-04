// Bugbot finding on PR #256: `type_mentions_param` only scanned the
// top-level tokens of a field's type, so a generic parameter buried inside
// a tuple, array, or parenthesized type (`(E, String)`) was invisible --
// both to `extra_bounds` (the `Display`/`Error` impl) and to
// `classify_bounds` (the `Classify` impl), which then added no bound at
// all for a param used only this way.
#[derive(Debug, errlanes::Classify)]
pub enum TupleField<E> {
    #[classify(fatal(Invariant))]
    #[error("failed: {0:?}")]
    Failed((E, String)),
}

fn main() {
    assert_eq!(
        TupleField::Failed((404, "not found".to_string())).to_string(),
        "failed: (404, \"not found\")"
    );
}
