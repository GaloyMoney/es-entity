#[derive(Debug, errlanes::Carrier)]
pub enum HostFault {
    Transient(errlanes::Transient),
    Sleepy(errlanes::Fatal),
}

fn main() {}
