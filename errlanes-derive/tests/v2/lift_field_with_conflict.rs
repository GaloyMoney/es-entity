#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, Clone)]
struct ConflictPayload {
    attempted: u32,
}
#[derive(Debug, Clone, errlanes::Rejection)]
enum Child {
    Conflict(ConflictPayload),
}
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child)]
enum Parent {
    #[rejection(code = "ATTEMPTED")]
    #[lift(Child::Conflict, field = attempted, with = transform)]
    Attempted(u32),
}
fn transform(_: Child) -> Parent {
    unreachable!()
}
fn main() {}
