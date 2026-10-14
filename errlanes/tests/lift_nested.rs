use errlanes::{Fail, FatalKind, Lift, ResultExt, lanes};
use std::error::Error;

#[derive(Debug, PartialEq)]
enum Leaf {
    C(u8),
    X,
}

#[derive(Debug, PartialEq)]
enum Mid {
    B(Leaf),
    Y,
}

#[derive(Debug, PartialEq, errlanes::Rejection)]
enum Outer {
    #[rejection(code = "A")]
    A(Mid),
    #[rejection(code = "Z")]
    Z,
}

fn is_even(value: &u8) -> bool {
    value.is_multiple_of(2)
}

fn is_big(value: &u8) -> bool {
    *value > 100
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Outer, unhandled = fatal)]
enum Plain {
    #[lift(Outer::A(Mid::B(Leaf::C)))]
    #[rejection(code = "NESTED_C")]
    C(u8),
}

fn nested(value: Leaf) -> Outer {
    Outer::A(Mid::B(value))
}

#[test]
fn a_nested_plain_arm_maps_its_leaf_and_returns_every_other_case_untouched() {
    assert!(matches!(Plain::lift(nested(Leaf::C(9))), Ok(Plain::C(9))));
    assert_eq!(Plain::lift(nested(Leaf::X)).unwrap_err(), nested(Leaf::X));
    assert_eq!(Plain::lift(Outer::A(Mid::Y)).unwrap_err(), Outer::A(Mid::Y));
    assert_eq!(Plain::lift(Outer::Z).unwrap_err(), Outer::Z);
}

#[test]
fn an_unmapped_nested_case_becomes_fatal_with_the_original_as_source() {
    let result: Result<(), Fail<Plain, lanes!(Fatal)>> = Err(Outer::A(Mid::Y)).lift();
    let Fail::Fatal(fatal) = result.unwrap_err() else {
        panic!("an unmapped case must become fatal")
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);
    assert_eq!(
        fatal.source().unwrap().downcast_ref::<Outer>(),
        Some(&Outer::A(Mid::Y))
    );
}

#[derive(Debug, PartialEq)]
struct Amount(u8);
impl From<u8> for Amount {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Outer, unhandled = fatal)]
enum Guarded {
    #[lift(Outer::A(Mid::B(Leaf::C)), when = is_big, into)]
    #[rejection(code = "BIG")]
    Big(Amount),
    #[lift(Outer::A(Mid::B(Leaf::C)), when = is_even, into)]
    #[rejection(code = "EVEN")]
    Even(Amount),
}

#[test]
fn two_guards_off_one_nested_leaf_route_separately_and_a_miss_is_fatal() {
    assert!(matches!(
        Guarded::lift(nested(Leaf::C(200))),
        Ok(Guarded::Big(Amount(200)))
    ));
    assert!(matches!(
        Guarded::lift(nested(Leaf::C(4))),
        Ok(Guarded::Even(Amount(4)))
    ));
    assert_eq!(
        Guarded::lift(nested(Leaf::C(3))).unwrap_err(),
        nested(Leaf::C(3))
    );
    let result: Result<(), Fail<Guarded, lanes!(Fatal)>> = Err(nested(Leaf::C(3))).lift();
    assert!(matches!(result, Err(Fail::Fatal(_))));
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Outer, unhandled = fatal)]
enum Mapped {
    #[lift(Outer::A(Mid::B(Leaf::C)), with = whole_outer)]
    #[rejection(code = "WHOLE")]
    Whole(Outer),
}

fn whole_outer(outer: Outer) -> Mapped {
    Mapped::Whole(outer)
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Outer, unhandled = fatal)]
enum MappedGuarded {
    #[lift(Outer::A(Mid::B(Leaf::C)), when = is_even, with = rebuilt)]
    #[rejection(code = "REBUILT")]
    Rebuilt(Outer),
}

fn rebuilt(outer: Outer) -> MappedGuarded {
    MappedGuarded::Rebuilt(outer)
}

#[test]
fn a_nested_with_arm_receives_the_whole_outer_value() {
    assert!(matches!(
        Mapped::lift(nested(Leaf::C(1))),
        Ok(Mapped::Whole(Outer::A(Mid::B(Leaf::C(1)))))
    ));
    assert_eq!(Mapped::lift(nested(Leaf::X)).unwrap_err(), nested(Leaf::X));
    assert!(matches!(
        MappedGuarded::lift(nested(Leaf::C(2))),
        Ok(MappedGuarded::Rebuilt(Outer::A(Mid::B(Leaf::C(2)))))
    ));
    assert_eq!(
        MappedGuarded::lift(nested(Leaf::C(3))).unwrap_err(),
        nested(Leaf::C(3))
    );
}

struct Reading {
    value: u8,
}
enum Meter {
    Read(Reading),
    Off,
}
enum Plant {
    Meter(Meter),
}

#[derive(errlanes::Lift)]
#[lift(Plant, unhandled = fatal)]
enum Projected {
    #[lift(Plant::Meter(Meter::Read), field = value)]
    Value(u8),
}

#[test]
fn a_nested_field_arm_projects_the_leaf_payload() {
    assert!(matches!(
        Projected::lift(Plant::Meter(Meter::Read(Reading { value: 7 }))),
        Ok(Projected::Value(7))
    ));
    assert!(matches!(
        Projected::lift(Plant::Meter(Meter::Off)),
        Err(Plant::Meter(Meter::Off))
    ));
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Outer, unhandled = fatal)]
enum Refined {
    #[lift(Outer::A(Mid::B(Leaf::C)), when = is_even)]
    #[rejection(code = "REFINED")]
    EvenLeaf(u8),
    #[lift(Outer::A(Mid::Y))]
    #[rejection(code = "Y")]
    Why,
    #[lift(Outer::A)]
    #[rejection(code = "ANY_A")]
    AnyA(Mid),
}

#[test]
fn a_flat_arm_after_nested_arms_catches_what_they_refuse() {
    assert!(matches!(
        Refined::lift(nested(Leaf::C(2))),
        Ok(Refined::EvenLeaf(2))
    ));
    assert!(matches!(Refined::lift(Outer::A(Mid::Y)), Ok(Refined::Why)));
    assert!(matches!(
        Refined::lift(nested(Leaf::C(3))),
        Ok(Refined::AnyA(Mid::B(Leaf::C(3))))
    ));
    assert_eq!(Refined::lift(Outer::Z).unwrap_err(), Outer::Z);
}
