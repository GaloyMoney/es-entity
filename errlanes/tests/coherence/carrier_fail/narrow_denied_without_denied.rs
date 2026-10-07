// `narrow_denied` on a carrier without a `Denied` lane.
use errlanes::ResultExt;

#[derive(Debug, errlanes::Carrier)]
enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn f(r: Result<(), HostFault>) {
    let _ = r.narrow_denied();
}

fn main() {
    f(Ok(()));
}
