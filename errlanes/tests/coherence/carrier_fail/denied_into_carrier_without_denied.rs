// `Denied` into a carrier that does not declare the lane.
#[errlanes::fault(Transient, Fatal)]
struct HostFault;

fn f() -> Result<(), HostFault> {
    Err(errlanes::Denied::new())?;
    Ok(())
}

fn main() {
    let _ = f();
}
