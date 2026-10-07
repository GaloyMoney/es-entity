// A rejection `?` into a fault-only carrier: no `Rejected` lane to take it.
#[derive(Debug, errlanes::Rejection)]
enum Closed {
    Yes,
}

#[errlanes::fault(Transient, Fatal)]
struct HostFault;

fn f() -> Result<(), HostFault> {
    Err(Closed::Yes)?;
    Ok(())
}

fn main() {
    let _ = f();
}
