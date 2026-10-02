// `from` builds the whole variant from the payload alone, so it needs the
// payload to be the only field; `delegate` may ignore siblings, `From`
// cannot invent them.
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[error("x")]
#[rejection(code = "X")]
struct Inner;

#[derive(Debug, thiserror::Error, errlanes::Classify)]
enum Wrapper {
    #[error("decode at {sequence}: {source}")]
    #[classify(delegate, from)]
    Decode {
        sequence: i32,
        #[source]
        source: Inner,
    },
}

fn main() {}
