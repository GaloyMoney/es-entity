#![allow(unused_imports)]
use errlanes::{Denied, Fail, Fatal, lanes};
#[derive(Debug, errlanes::Rejection)]
enum Child {
    One(u32),
    Two,
}
fn main() {
    // A profile with Transient but no Fatal has nowhere to put an exhausted
    // retry, so it cannot be narrowed (and `Laned`, hence `retry`, excludes it).
    let _ = Fail::<Child, lanes!(Transient)>::Transient(errlanes::Transient::new(
        errlanes::TransientKind::Deadlock,
    ))
    .narrow_transient(2);
}
