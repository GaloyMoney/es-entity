// A fail-like carrier `?` into a fault-only carrier.
#[derive(Debug, errlanes::Rejection)]
enum Closed {
    Yes,
}

#[derive(Debug, errlanes::Carrier)]
enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

#[derive(Debug, errlanes::Carrier)]
enum CustomerError {
    Rejected(Closed),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn f(r: Result<(), CustomerError>) -> Result<(), HostFault> {
    r?;
    Ok(())
}

fn main() {
    let _ = f(Ok(()));
}
