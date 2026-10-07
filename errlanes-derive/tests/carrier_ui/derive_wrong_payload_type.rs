// The error must point at the field type, not at the derive.
#[derive(Debug, errlanes::Carrier)]
pub enum HostFault {
    Transient(String),
    Fatal(errlanes::Fatal),
}

fn main() {}
