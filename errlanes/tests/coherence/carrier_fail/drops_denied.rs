// A carrier with `Denied` `?` into a `Fault` that drops it.
use errlanes::{Fault, lanes};

#[errlanes::fault(Denied, Transient, Fatal)]
struct PartyFault;

fn f(r: Result<(), PartyFault>) -> Result<(), Fault<lanes!(Transient, Fatal)>> {
    r?;
    Ok(())
}

fn main() {
    let _ = f(Ok(()));
}
