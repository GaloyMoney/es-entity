// `Denied` into a `Fault` that does not declare the lane.
use errlanes::{Fault, lanes};
use zerocopy as _;

fn f() -> Result<(), Fault<lanes!(Transient, Fatal)>> {
    Err(errlanes::Denied::new())?;
    Ok(())
}

fn main() {
    let _ = f();
}
