use errlanes::{Fault, lanes};
use zerocopy as _;

fn outer(inner: Result<(), Fault<lanes!(Denied, Fatal)>>) -> Result<(), Fault<lanes!(Fatal)>> {
    inner?;
    Ok(())
}
fn main() { let _ = outer(Ok(())); }
