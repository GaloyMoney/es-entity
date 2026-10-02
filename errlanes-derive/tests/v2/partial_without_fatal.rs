#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, errlanes::Rejection)]
enum Child {
    One(u32),
    Two,
}
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, unhandled = fatal)]
enum Parent { #[lift(Child::One)] One(u32) }
fn main() { let source: Fail<Child, lanes!()> = Fail::Rejected(Child::Two); let _: Fail<Parent, lanes!()> = source.lift(); }
