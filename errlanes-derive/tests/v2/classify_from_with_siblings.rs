// `from` builds the whole variant from the payload alone, so it needs the
// payload to be the only field; `delegate` may ignore siblings, `From`
// cannot invent them.
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "X")]
struct Inner;

#[derive(Debug, errlanes::Classify)]
enum Wrapper {
    #[classify(delegate, from)]
    Decode {
        sequence: i32,
        #[source]
        source: Inner,
    },
}

fn main() {}
