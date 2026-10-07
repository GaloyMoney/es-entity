// A rejection `?` into a fault-only carrier: no `Rejected` lane to take it.
#[derive(Debug, errlanes::Rejection)]
enum Closed {
    Yes,
}

#[derive(Debug, errlanes::Carrier)]
enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn f() -> Result<(), HostFault> {
    Err(Closed::Yes)?;
    Ok(())
}

fn main() {
    let _ = f();
}
