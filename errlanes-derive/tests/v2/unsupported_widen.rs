#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Child {
    #[error("one {0}")] One(u32),
    #[error("two")] Two,
}
fn main() { let source: Fail<Child> = Fail::Denied(Denied::default()); let _: Fail<Child, lanes!(Fatal)> = source.widen(); }
