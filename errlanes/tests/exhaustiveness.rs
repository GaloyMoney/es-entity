//! A by-value match names exactly the lanes a profile enables: the arms for
//! disabled slots are not just unnecessary, they are inexpressible. A borrowed
//! match is the exception, which the `as_*` accessors cover instead.
use errlanes::{Fail, Fatal, FatalKind, Fault, Settled, Transient, TransientKind, lanes};

#[derive(Debug, Clone, thiserror::Error)]
#[error("nope")]
struct Nope;
impl errlanes::Rejection for Nope {
    type Code = NopeCode;
    fn code(&self) -> NopeCode {
        NopeCode::Nope
    }
}
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

// (1) exactly the two named arms, by value
fn two_arms(f: Fault<lanes!(Transient, Fatal)>) -> &'static str {
    match f {
        Fault::Transient(_) => "transient",
        Fault::Fatal(_) => "fatal",
    }
}

// (2) one named arm, by value
fn one_arm(f: Fault<lanes!(Fatal)>) -> Fatal {
    match f {
        Fault::Fatal(x) => x,
    }
}

// (3) irrefutable let for a single-lane fault
fn one_arm_let(f: Fault<lanes!(Fatal)>) -> Fatal {
    let Fault::Fatal(x) = f;
    x
}

// (4) Fail: Rejected plus exactly the named fault lanes
fn fail_arms(f: Fail<Nope, lanes!(Transient, Fatal)>) -> &'static str {
    match f {
        Fail::Rejected(_) => "rejected",
        Fail::Transient(_) => "transient",
        Fail::Fatal(_) => "fatal",
    }
}

// (5) Fail with no fault lanes at all: one arm
fn fail_rejected_only(f: Fail<Nope, lanes!()>) -> Nope {
    let Fail::Rejected(r) = f;
    r
}

// (6) A settled profile is just `Fail` with the transient slot off: the arms
// are the lanes that remain, and exhaustion is in the fatal lane.
fn settled_arms(f: Fail<Nope, Settled<lanes!(Transient, Fatal)>>) -> &'static str {
    match f {
        Fail::Rejected(_) => "rejected",
        Fail::Fatal(f) if f.kind == FatalKind::Exhausted => "exhausted",
        Fail::Fatal(_) => "fatal",
    }
}

#[test]
fn by_value_matches_need_only_the_named_lanes() {
    assert_eq!(
        two_arms(Transient::new(TransientKind::Deadlock).into()),
        "transient"
    );
    assert_eq!(
        one_arm(Fatal::invariant("x").into()).kind,
        FatalKind::Invariant
    );
    assert_eq!(
        one_arm_let(Fatal::invariant("x").into()).kind,
        FatalKind::Invariant
    );
    assert_eq!(fail_arms(Fail::Rejected(Nope)), "rejected");
    let _: Nope = fail_rejected_only(Fail::Rejected(Nope));
    assert_eq!(settled_arms(Fail::Rejected(Nope)), "rejected");
    let t: Fail<Nope, lanes!(Transient, Fatal)> = Transient::new(TransientKind::Deadlock).into();
    assert_eq!(settled_arms(t.settle(3)), "exhausted");
}

// (7) borrowed inspection without a dead arm
fn borrowed(e: &Fault<lanes!(Transient, Fatal)>) -> &'static str {
    if e.as_transient().is_some() {
        "transient"
    } else if e.as_fatal().is_some() {
        "fatal"
    } else {
        unreachable!()
    }
}

// (8) the REFERENCE example, without any match at all
fn lane_of(e: &Fault<lanes!(Fatal)>) -> errlanes::Lane {
    e.lane()
}

#[test]
fn borrowed_accessors_replace_borrowed_matches() {
    let t: Fault<lanes!(Transient, Fatal)> = Transient::new(TransientKind::Deadlock).into();
    assert_eq!(borrowed(&t), "transient");
    assert_eq!(t.as_transient().unwrap().kind, TransientKind::Deadlock);
    assert!(t.as_fatal().is_none());
    let f: Fault<lanes!(Fatal)> = Fatal::invariant("x").into();
    assert_eq!(lane_of(&f), errlanes::Lane::Fatal);
    let r: Fail<Nope, lanes!(Transient, Fatal)> = Fail::Rejected(Nope);
    assert!(r.as_rejected().is_some());
    assert!(r.as_transient().is_none());
}
