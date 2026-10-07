// Carrier -> carrier by `?` needs `from(..)`: the reflexive `From<T> for T`
// is why it cannot be automatic.
#[derive(Debug, errlanes::Carrier)]
enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

#[derive(Debug, errlanes::Carrier)]
enum RepoFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn f(r: Result<(), HostFault>) -> Result<(), RepoFault> {
    r?;
    Ok(())
}

fn main() {
    let _ = f(Ok(()));
}
