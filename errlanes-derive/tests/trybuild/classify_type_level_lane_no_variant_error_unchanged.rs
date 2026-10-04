// Scoping check for the Finding-1 fix: a type-level-laned enum with *no*
// per-variant `#[error(..)]` at all must keep rendering exactly what it
// rendered before the fix -- the snake_cased type name, uniformly, for
// every variant.
#[derive(Debug, errlanes::Classify)]
#[classify(fatal(Invariant))]
enum PlainWrapper {
    First,
    Second(String),
}

fn main() {
    assert_eq!(PlainWrapper::First.to_string(), "plain_wrapper");
    assert_eq!(
        PlainWrapper::Second("x".to_string()).to_string(),
        "plain_wrapper"
    );
}
