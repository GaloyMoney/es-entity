// `?` can add lanes but never discard one.
use errlanes::{Fault, ResultExt, lanes};
use zerocopy as _;

#[derive(Debug, errlanes::Carrier)]
enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn f(r: Result<(), HostFault>) -> Result<(), Fault<lanes!(Fatal)>> {
    r.into_fault()?;
    Ok(())
}

fn main() {
    let _ = f(Ok(()));
}
