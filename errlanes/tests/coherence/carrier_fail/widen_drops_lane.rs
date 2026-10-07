// `.widen()` can add lanes but never discard one.
use errlanes::{Fault, ResultExt, lanes};

#[derive(Debug, errlanes::Carrier)]
enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn f(r: Result<(), HostFault>) -> Result<(), Fault<lanes!(Fatal)>> {
    r.widen::<Fault<lanes!(Fatal)>>()
}

fn main() {
    let _ = f(Ok(()));
}
