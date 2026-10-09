use errlanes::{Fail, lanes};

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "CHILD")]
struct Child;
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "PARENT")]
struct Parent;
impl From<Child> for Parent {
    fn from(_: Child) -> Self { Parent }
}

fn outer(inner: Result<(), Fail<Child, lanes!(Fatal)>>) -> Result<(), Fail<Parent, lanes!(Fatal)>> {
    inner?;
    Ok(())
}
fn main() { let _ = outer(Ok(())); }
