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
    #[lift(Child::Conflict, field = attempted)]
    Attempted(u32, u32),
}
fn main() {}
