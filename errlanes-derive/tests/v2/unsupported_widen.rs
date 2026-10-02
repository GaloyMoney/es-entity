#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, errlanes::Rejection)]
enum Child {
    One(u32),
    Two,
}
fn main() { let source: Fail<Child> = Fail::Denied(Denied::default()); let _: Fail<Child, lanes!(Fatal)> = source.widen(); }
