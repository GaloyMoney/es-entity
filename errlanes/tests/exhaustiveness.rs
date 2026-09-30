//! A by-value match names exactly the lanes a profile enables: the arms for
//! disabled slots are not just unnecessary, they are inexpressible. A borrowed
//! match is the exception, which the `as_*` accessors cover instead.
use errlanes::{Fail, Fatal, FatalKind, Fault, Lane, Transient, TransientKind, lanes};

#[derive(Debug, Clone, thiserror::Error)]
#[error("nope")]
struct Nope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum NopeCode {
    Nope,
}

impl std::fmt::Display for NopeCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NOPE")
    }
}

impl From<NopeCode> for &'static str {
    fn from(_: NopeCode) -> Self {
        "NOPE"
    }
}

impl errlanes::Rejection for Nope {
    type Code = NopeCode;
    fn code(&self) -> NopeCode {
        NopeCode::Nope
    }
}

fn two_lanes(failure: Fault<lanes!(Transient, Fatal)>) -> &'static str {
    match failure {
        Fault::Transient(_) => "transient",
        Fault::Fatal(_) => "fatal",
    }
}

fn one_lane(failure: Fault<lanes!(Fatal)>) -> Fatal {
    match failure {
        Fault::Fatal(fatal) => fatal,
    }
}

/// A single inhabited lane destructures irrefutably.
fn one_lane_let(failure: Fault<lanes!(Fatal)>) -> Fatal {
    let Fault::Fatal(fatal) = failure;
    fatal
}

fn rejected_plus_two(failure: Fail<Nope, lanes!(Transient, Fatal)>) -> &'static str {
    match failure {
        Fail::Rejected(_) => "rejected",
        Fail::Transient(_) => "transient",
        Fail::Fatal(_) => "fatal",
    }
}

/// `lanes!()` selects no fault lanes, leaving only the rejection.
fn rejected_only(failure: Fail<Nope, lanes!()>) -> Nope {
    let Fail::Rejected(rejection) = failure;
    rejection
}

#[test]
fn a_by_value_match_names_only_the_selected_lanes() {
    assert_eq!(
        two_lanes(Transient::new(TransientKind::Deadlock).into()),
        "transient"
    );
    assert_eq!(two_lanes(Fatal::invariant("x").into()), "fatal");
    assert_eq!(
        one_lane(Fatal::invariant("x").into()).kind,
        FatalKind::Invariant
    );
    assert_eq!(
        one_lane_let(Fatal::invariant("x").into()).kind,
        FatalKind::Invariant
    );
    assert_eq!(rejected_plus_two(Fail::Rejected(Nope)), "rejected");
    let _: Nope = rejected_only(Fail::Rejected(Nope));
}

/// Borrowed inspection goes through the accessors, not a match.
fn inspect(failure: &Fault<lanes!(Transient, Fatal)>) -> Lane {
    if let Some(transient) = failure.as_transient() {
        assert_eq!(transient.kind, TransientKind::Deadlock);
    }
    failure.lane()
}

#[test]
fn borrowed_accessors_replace_borrowed_matches() {
    let transient: Fault<lanes!(Transient, Fatal)> = Transient::new(TransientKind::Deadlock).into();
    assert_eq!(inspect(&transient), Lane::Transient);
    assert!(transient.as_fatal().is_none());

    let fatal: Fault<lanes!(Fatal)> = Fatal::invariant("x").into();
    assert_eq!(fatal.lane(), Lane::Fatal);
    assert!(fatal.as_fatal().is_some());

    let rejected: Fail<Nope, lanes!(Transient, Fatal)> = Fail::Rejected(Nope);
    assert!(rejected.as_rejected().is_some());
    assert!(rejected.as_transient().is_none());
}
