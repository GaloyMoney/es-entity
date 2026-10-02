#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, errlanes::Rejection)]
enum Child {
    One(u32),
    Two,
}
fn main() { let _: Fail<Child, lanes!(Fatal)> = Denied::default().into(); }
