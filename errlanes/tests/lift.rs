use errlanes::{Fail, Fault, ResultExt, lanes, profile::Profile};

#[derive(Debug, errlanes::Rejection)]
enum Child {
    Mapped,
    Unmapped,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, unhandled = fatal)]
enum Parent {
    #[lift(Child::Mapped)]
    Mapped,
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "PARENT_TOTAL")]
struct ParentTotal;

impl From<Child> for ParentTotal {
    fn from(_: Child) -> Self {
        Self
    }
}

#[derive(Debug, errlanes::Carrier)]
enum ParentCarrier {
    Rejected(Parent),
    Fatal(errlanes::Fatal),
}

fn bare(child: Child) -> Result<(), Child> {
    Err(child)
}

fn into_fail(child: Child) -> Result<(), Fail<Parent, lanes!(Transient, Fatal)>> {
    bare(child).lift()?;
    Ok(())
}

fn into_carrier(child: Child) -> Result<(), ParentCarrier> {
    bare(child).lift()?;
    Ok(())
}

fn total_without_fatal() -> Result<(), Fail<ParentTotal, lanes!()>> {
    bare(Child::Mapped)?;
    Ok(())
}

#[test]
fn partial_bare_lift_uses_the_fatal_lane_and_infers_into_a_carrier() {
    assert!(matches!(
        into_fail(Child::Mapped),
        Err(Fail::Rejected(Parent::Mapped))
    ));
    assert!(matches!(into_fail(Child::Unmapped), Err(Fail::Fatal(_))));
    assert!(matches!(
        into_carrier(Child::Mapped),
        Err(ParentCarrier::Rejected(Parent::Mapped))
    ));
    assert!(matches!(
        into_carrier(Child::Unmapped),
        Err(ParentCarrier::Fatal(_))
    ));
    assert!(matches!(
        total_without_fatal(),
        Err(Fail::Rejected(ParentTotal))
    ));
}

fn from_exists<S, D: From<S>>() {}

/// All 19 strict profile inclusions must work for both built-in shapes.
#[test]
fn every_strict_lane_inclusion_has_from() {
    macro_rules! pair {
        (($sd:literal,$st:literal,$sf:literal) => ($dd:literal,$dt:literal,$df:literal)) => {
            from_exists::<Fault<Profile<$sd, $st, $sf>>, Fault<Profile<$dd, $dt, $df>>>();
            from_exists::<Fail<Child, Profile<$sd, $st, $sf>>, Fail<Child, Profile<$dd, $dt, $df>>>(
            );
        };
    }
    pair!((false,false,false) => (true,false,false));
    pair!((false,false,false) => (false,true,false));
    pair!((false,false,false) => (false,false,true));
    pair!((false,false,false) => (true,true,false));
    pair!((false,false,false) => (true,false,true));
    pair!((false,false,false) => (false,true,true));
    pair!((false,false,false) => (true,true,true));
    pair!((true,false,false) => (true,true,false));
    pair!((true,false,false) => (true,false,true));
    pair!((true,false,false) => (true,true,true));
    pair!((false,true,false) => (true,true,false));
    pair!((false,true,false) => (false,true,true));
    pair!((false,true,false) => (true,true,true));
    pair!((false,false,true) => (true,false,true));
    pair!((false,false,true) => (false,true,true));
    pair!((false,false,true) => (true,true,true));
    pair!((true,true,false) => (true,true,true));
    pair!((true,false,true) => (true,true,true));
    pair!((false,true,true) => (true,true,true));
}
