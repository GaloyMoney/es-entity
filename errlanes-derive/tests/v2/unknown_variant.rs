#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, errlanes::Rejection)]
enum Child {
    One(u32),
    Two,
}
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, unhandled = fatal)]
enum Parent { #[lift(Child::Missing)] One }
fn main() {}
