#![allow(unused_imports)]
use errlanes::{Denied, Fail, Fatal, lanes};
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Child {
    #[error("one {0}")]
    One(u32),
    #[error("two")]
    Two,
}
fn main() {
    // A profile with Transient but no Fatal has nowhere to put an exhausted
    // retry, so it cannot be settled (and `Laned`, hence `retry`, excludes it).
    let _ = Fail::<Child, lanes!(Transient)>::Transient(errlanes::Transient::new(
        errlanes::TransientKind::Deadlock,
    ))
    .settle(2);
}
