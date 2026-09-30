#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Child {
    #[error("one {0}")] One(u32),
    #[error("two")] Two,
}
#[errlanes::rejection]
#[derive(Debug, thiserror::Error, errlanes::Lift)]
enum Parent { #[flatten] Child(Child), #[error("one")] One }
fn main() {}
