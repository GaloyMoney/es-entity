#[derive(Debug, errlanes::Carrier)]
pub enum HostFault {
    Fatal { inner: errlanes::Fatal },
}

fn main() {}
