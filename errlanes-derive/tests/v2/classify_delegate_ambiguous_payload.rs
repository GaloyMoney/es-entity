// `delegate` on a variant with several fields needs the payload singled out,
// the way thiserror singles out a cause: `#[source]`, `#[from]`, or a field
// named `source`. Two unmarked fields are ambiguous.
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[error("x")]
#[rejection(code = "X")]
struct Inner;

#[derive(Debug, thiserror::Error, errlanes::Classify)]
enum Wrapper {
    #[error("x")]
    #[classify(delegate)]
    Delegated { first: Inner, second: Inner },
}

fn main() {}
