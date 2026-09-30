#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Child {
    #[error("one {0}")] One(u32),
    #[error("two")] Two,
}
#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, unhandled = fatal)]
enum Parent { #[error("one {0}")] #[lift(Child::One)] One(u32) }
fn main() { let source: Fail<Child, lanes!()> = Fail::Rejected(Child::Two); let _: Fail<Parent, lanes!()> = source.lift(); }
