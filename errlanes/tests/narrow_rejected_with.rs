use errlanes::{Fail, Fatal, FatalKind, Fault, ResultExt, lanes};

use std::{error::Error, fmt};

#[derive(Debug, errlanes::Rejection)]
enum Outcome {
    Missing,
    Corrupt,
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "SECRET", error = manual)]
struct Secret;

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("secret-sentinel")
    }
}

impl Error for Secret {}

type Lanes = lanes!(Transient, Fatal);

fn narrow(value: Result<u32, Fail<Outcome, Lanes>>) -> Result<u32, Fault<Lanes>> {
    value.narrow_rejected_with(|rejected| match rejected {
        Outcome::Missing => Ok(0),
        Outcome::Corrupt => Err(rejected),
    })
}

#[test]
fn ok_passes_through_unchanged() {
    assert_eq!(narrow(Ok(1)).unwrap(), 1);
}

#[test]
fn a_member_recovered_by_the_closure_yields_its_value() {
    assert_eq!(narrow(Err(Fail::Rejected(Outcome::Missing))).unwrap(), 0);
}

#[test]
fn a_member_handed_back_is_narrowed_to_an_invariant() {
    let err = narrow(Err(Fail::Rejected(Outcome::Corrupt))).unwrap_err();
    match err {
        Fault::Fatal(fatal) => {
            assert_eq!(fatal.kind, FatalKind::Invariant);
            assert!(matches!(
                fatal.source().unwrap().downcast_ref::<Outcome>(),
                Some(Outcome::Corrupt)
            ));
        }
        other => panic!("expected Fatal(Invariant), got {other:?}"),
    }
}

#[test]
fn faults_pass_through_without_running_the_closure() {
    let value: Result<u32, Fail<Outcome, Lanes>> =
        Err(Fail::Fatal(Fatal::new(FatalKind::Invariant)));
    let err = value.narrow_rejected_with(|_| unreachable!()).unwrap_err();
    assert!(matches!(err, Fault::Fatal(_)));
}

#[test]
fn a_member_handed_back_does_not_leak_its_display() {
    let value: Result<(), Fail<Secret, Lanes>> = Err(Fail::Rejected(Secret));
    let err = value.narrow_rejected_with(Err).unwrap_err();
    match &err {
        Fault::Fatal(fatal) => {
            assert!(fatal.source().unwrap().downcast_ref::<Secret>().is_some());
        }
        other => panic!("expected Fatal(Invariant), got {other:?}"),
    }
    let message = err.message();
    assert!(!message.contains("secret-sentinel"), "{message:?}");
    let display = err.to_string();
    assert!(!display.contains("secret-sentinel"), "{display:?}");
}
