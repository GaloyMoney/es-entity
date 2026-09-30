#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Child {
    #[error("one {0}")] One(u32),
    #[error("two")] Two,
}
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Parent { #[error("{0}")] #[rejection(delegate, code = "NEW")] Child(Child) }
fn main() {}
