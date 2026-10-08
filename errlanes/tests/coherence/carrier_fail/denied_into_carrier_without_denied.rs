// `Denied` into a carrier that does not declare the lane.
use zerocopy as _;

#[derive(Debug, errlanes::Carrier)]
enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn f() -> Result<(), HostFault> {
    Err(errlanes::Denied::new())?;
    Ok(())
}

fn main() {
    let _ = f();
}
