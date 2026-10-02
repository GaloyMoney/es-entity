#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, errlanes::Rejection)]
enum Child {
    One(u32),
    Two,
}
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child)]
enum Parent {
    #[lift(Child::One)] One(u32),
    #[lift(Child::One)] Other(u32),
}
fn main() {}
