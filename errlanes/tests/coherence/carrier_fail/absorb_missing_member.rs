use errlanes::{Fail, Fault, ResultExt, lanes};
use zerocopy as _;

#[derive(Debug, errlanes::Rejection)]
enum Outcome {
    Missing,
    Corrupt,
}

fn absorb(value: Result<(), Fail<Outcome, lanes!(Fatal)>>) -> Result<(), Fault<lanes!(Fatal)>> {
    value.absorb(
        |()| (),
        |rejected| match rejected {
            Outcome::Missing => Ok(()),
        },
    )
}
fn main() {
    let _ = absorb(Ok(()));
}
