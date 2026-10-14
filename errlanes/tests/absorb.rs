use errlanes::{Fail, Fatal, FatalKind, Fault, ResultExt, lanes};

#[derive(Debug, errlanes::Rejection)]
enum Outcome {
    Missing,
    Corrupt,
}

type Lanes = lanes!(Transient, Fatal);

fn absorb(value: Result<u32, Fail<Outcome, Lanes>>) -> Result<u32, Fault<Lanes>> {
    value.absorb(
        |n| n + 1,
        |rejected| match rejected {
            Outcome::Missing => Ok(0),
            Outcome::Corrupt => {
                Err(Fatal::from_error(FatalKind::Invariant, Outcome::Corrupt).into())
            }
        },
    )
}

#[test]
fn ok_is_mapped_by_on_ok() {
    assert_eq!(absorb(Ok(1)).unwrap(), 2);
}

#[test]
fn a_member_absorbed_by_the_closure_yields_its_value() {
    assert_eq!(absorb(Err(Fail::Rejected(Outcome::Missing))).unwrap(), 0);
}

#[test]
fn a_member_the_closure_faults_on_becomes_that_fault() {
    let err = absorb(Err(Fail::Rejected(Outcome::Corrupt))).unwrap_err();
    assert!(matches!(err, Fault::Fatal(_)));
}

#[test]
fn faults_pass_through_without_running_the_closures() {
    let value: Result<u32, Fail<Outcome, Lanes>> =
        Err(Fail::Fatal(Fatal::new(FatalKind::Invariant)));
    let err = value
        .absorb(|_| unreachable!(), |_| unreachable!())
        .unwrap_err();
    assert!(matches!(err, Fault::Fatal(_)));
}
