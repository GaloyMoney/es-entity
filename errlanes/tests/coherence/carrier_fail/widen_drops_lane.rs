// `.widen()` can add lanes but never discard one.
use errlanes::{Fault, ResultExt, lanes};

#[errlanes::fault(Transient, Fatal)]
struct HostFault;

fn f(r: Result<(), HostFault>) -> Result<(), Fault<lanes!(Fatal)>> {
    r.widen::<Fault<lanes!(Fatal)>>()
}

fn main() {
    let _ = f(Ok(()));
}
