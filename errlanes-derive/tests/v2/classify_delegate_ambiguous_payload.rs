// `delegate` on a variant with several fields needs the payload singled out:
// `#[source]`, or a field named `source`. Two unmarked fields are ambiguous.
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "X")]
struct Inner;

#[derive(Debug, errlanes::Classify)]
enum Wrapper {
    #[classify(delegate)]
    Delegated { first: Inner, second: Inner },
}

fn main() {}
