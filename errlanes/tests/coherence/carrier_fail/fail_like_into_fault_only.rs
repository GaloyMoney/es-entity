// A fail-like carrier `?` into a fault-only carrier.
#[derive(Debug, errlanes::Rejection)]
enum Closed {
    Yes,
}

#[errlanes::fault(Transient, Fatal)]
struct HostFault;

#[errlanes::fail(Closed; Transient, Fatal)]
struct CustomerError;

fn f(r: Result<(), CustomerError>) -> Result<(), HostFault> {
    r?;
    Ok(())
}

fn main() {
    let _ = f(Ok(()));
}
