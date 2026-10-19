// A `Fail` `?` into a fault-only `Fault`: the rejection has nowhere to go.
use errlanes::{Fail, Fault, lanes};

#[derive(Debug, errlanes::Rejection)]
enum Closed {
    Yes,
}

fn f(r: Result<(), Fail<Closed, lanes!(Transient, Fatal)>>) -> Result<(), Fault<lanes!(Transient, Fatal)>> {
    r?;
    Ok(())
}

fn main() {
    let _ = f(Ok(()));
}
