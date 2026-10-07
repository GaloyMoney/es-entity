#[derive(Debug, errlanes::Carrier)]
#[non_exhaustive]
pub enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn main() {}
