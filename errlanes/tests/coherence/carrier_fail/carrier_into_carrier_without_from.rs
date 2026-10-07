// Carrier -> carrier by `?` needs `from(..)`: the reflexive `From<T> for T`
// is why it cannot be automatic.
#[errlanes::fault(Transient, Fatal)]
struct HostFault;

#[errlanes::fault(Transient, Fatal)]
struct RepoFault;

fn f(r: Result<(), HostFault>) -> Result<(), RepoFault> {
    r?;
    Ok(())
}

fn main() {
    let _ = f(Ok(()));
}
