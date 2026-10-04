// Trailing arguments are restricted to field access paths (`field.member`);
// an arbitrary expression, like a method call, is rejected rather than
// accepted the way `thiserror` would.
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "X")]
#[error("bad: {}", id.to_string())]
struct Bad {
    id: i32,
}

fn main() {}
