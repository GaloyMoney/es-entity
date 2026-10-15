use errlanes::{Fail, Fault, ResultExt, lanes};
use zerocopy as _;

#[derive(Debug, errlanes::Rejection)]
enum Outcome {
    Missing,
    Corrupt,
}

fn narrow(value: Result<(), Fail<Outcome, lanes!(Fatal)>>) -> Result<(), Fault<lanes!(Fatal)>> {
    value.narrow_rejected_with(|rejected| match rejected {
        Outcome::Missing => Ok(()),
    })
}
fn main() {
    let _ = narrow(Ok(()));
}
