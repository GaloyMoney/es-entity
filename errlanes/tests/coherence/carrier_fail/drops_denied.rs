// A carrier with `Denied` `?` into a `Fault` that drops it.
use errlanes::{Fault, lanes};
use zerocopy as _;

#[derive(Debug, errlanes::Carrier)]
enum PartyFault {
    Denied(errlanes::Denied),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn f(r: Result<(), PartyFault>) -> Result<(), Fault<lanes!(Transient, Fatal)>> {
    r?;
    Ok(())
}

fn main() {
    let _ = f(Ok(()));
}
