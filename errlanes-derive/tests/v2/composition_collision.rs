#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Child {
    #[error("one {0}")] One(u32),
    #[error("two")] Two,
}
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
enum Parent { #[compose(flatten)] Child(Child), #[error("one")] ChildOne }
fn main() {}
