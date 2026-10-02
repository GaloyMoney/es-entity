#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, errlanes::Rejection)]
enum Child {
    One(u32),
    Two,
}
#[errlanes::compose]
#[derive(Debug)]
enum Parent { #[compose(flatten)] Child(Child), ChildOne }
fn main() {}
