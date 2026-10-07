// `narrow_denied` on a carrier without a `Denied` lane.
use errlanes::ResultExt;

#[errlanes::fault(Transient, Fatal)]
struct HostFault;

fn f(r: Result<(), HostFault>) {
    let _ = r.narrow_denied();
}

fn main() {
    f(Ok(()));
}
