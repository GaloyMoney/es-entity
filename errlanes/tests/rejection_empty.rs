#![deny(warnings)]

use errlanes::{Fail, lanes};

/// A rejection with no members: what a repo whose writes can reject on
/// nothing generates. Deriving on it must compile without warnings, and the
/// carrier over it has only fault lanes to inhabit.
#[derive(Debug, errlanes::Rejection)]
enum Never {}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Never)]
enum Wider {
    #[rejection(code = "OTHER")]
    Other,
}

#[test]
fn an_empty_rejection_derives_and_its_carrier_is_always_a_fault() {
    let fail: Fail<Never, lanes!(Fatal)> = Fail::Fatal(errlanes::Fatal::invariant("boom"));
    assert!(fail.is_fatal());
}

#[test]
fn a_wider_rejection_lifts_the_empty_one() {
    fn lift(never: Never) -> Wider {
        Wider::from(never)
    }
    let _ = lift;
    assert!(matches!(Wider::Other, Wider::Other));
}
