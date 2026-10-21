#![deny(warnings)]

use errlanes::{Fail, ResultExt, lanes};
use zerocopy as _;

#[derive(Debug, errlanes::Rejection)]
enum Child {
    A,
    B,
}
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, unhandled = fatal)]
enum Parent {
    #[lift(Child::A)]
    A,
    #[lift(Child::B)]
    B,
}

fn outer(inner: Result<(), Child>) -> Result<(), Fail<Parent, lanes!(Fatal)>> {
    inner.lift()?;
    Ok(())
}
fn main() {
    let _ = outer(Ok(()));
}
