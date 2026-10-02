#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, errlanes::Rejection)]
enum Child {
    One(u32),
    Two,
}
#[derive(Debug, errlanes::Rejection)]
enum Parent { #[rejection(delegate, code = "NEW")] Child(Child) }
fn main() {}
