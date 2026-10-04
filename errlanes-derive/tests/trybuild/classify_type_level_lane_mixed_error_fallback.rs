// A type-level `#[error(..)]` still serves as the fallback for a variant
// that has no `#[error(..)]` of its own; a variant with its own `#[error]`
// overrides it. This was the pre-fix behaviour for the "no per-variant
// `#[error]` at all" case, generalized to "no per-variant `#[error]` on
// *this* variant".
#[derive(Debug, errlanes::Classify)]
#[classify(fatal(Invariant))]
#[error("fallback message")]
enum Mixed {
    #[error("specific message")]
    Specific,
    Generic,
}

fn main() {
    assert_eq!(Mixed::Specific.to_string(), "specific message");
    assert_eq!(Mixed::Generic.to_string(), "fallback message");
}
