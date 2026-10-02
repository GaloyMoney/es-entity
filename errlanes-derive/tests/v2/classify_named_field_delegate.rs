// A named single field is not a tuple field: `delegate`/`from` build a tuple
// constructor/pattern (`Self(value)`/`Self::Variant(p)`), which this shape
// cannot satisfy. Caught with a spanned error rather than emitting
// unparsable generated code.
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[error("x")]
#[rejection(code = "X")]
struct Inner;

#[derive(Debug, thiserror::Error, errlanes::Classify)]
enum Wrapper {
    #[error("x")]
    #[classify(delegate)]
    Delegated { source: Inner },
}

fn main() {}
