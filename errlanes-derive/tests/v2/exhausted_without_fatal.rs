#![allow(unused_imports)]
use errlanes::{Fail, Fatal, Denied, lanes};
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Child {
    #[error("one {0}")] One(u32),
    #[error("two")] Two,
}
fn main() {
    let settled = Fail::<Child, lanes!(Transient)>::Transient(errlanes::Transient::new(errlanes::TransientKind::Deadlock)).settle(2);
    let _: Fail<Child, lanes!(Transient)> = settled.into_fail();
}
