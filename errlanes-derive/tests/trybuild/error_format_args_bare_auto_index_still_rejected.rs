// With no trailing arguments, auto-indexed `{}` stays rejected: today's
// diagnostic is unchanged, since nothing could ever supply a positional
// argument for it to bind to.
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "X")]
#[error("bad {}")]
struct Bad {
    id: i32,
}

fn main() {}
