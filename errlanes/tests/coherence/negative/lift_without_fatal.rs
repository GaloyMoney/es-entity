use errlanes::{Fail, ResultExt, lanes};
use zerocopy as _;

#[derive(Debug, errlanes::Rejection)]
enum Child { Mapped, Unmapped }
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, unhandled = fatal)]
enum Parent { #[lift(Child::Mapped)] Mapped }

fn outer(inner: Result<(), Child>) -> Result<(), Fail<Parent, lanes!()>> {
    inner.lift()?;
    Ok(())
}
fn main() { let _ = outer(Ok(())); }
