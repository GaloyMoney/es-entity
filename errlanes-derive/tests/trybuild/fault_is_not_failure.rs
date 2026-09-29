// `Fault` has no domain content, so it cannot implement `Failure` (which
// requires an associated `Rejection`) — a `Failure` bound on it is a
// compile error, not a silent no-op.
fn wants_failure<F: errlanes::Failure>() {}

fn main() {
    wants_failure::<errlanes::Fault>();
}
