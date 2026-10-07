// The attribute emits `Debug` itself; deriving it again is a duplicate impl.
#[errlanes::fault(Transient, Fatal)]
#[derive(Debug)]
pub struct HostFault;

fn main() {}
