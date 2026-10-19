#![deny(warnings)]

use errlanes::{Fail, FatalKind, Lift, ResultExt, lanes};
use std::error::Error;

#[derive(Debug, PartialEq, errlanes::Rejection)]
enum Flat {
    #[rejection(code = "FLAT_A")]
    A,
    #[rejection(code = "FLAT_B")]
    B,
    #[rejection(code = "FLAT_C")]
    C,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Flat, fatal(B, C))]
enum FlatMapped {
    #[lift(Flat::A)]
    A,
}

#[test]
fn listed_cases_return_the_source_and_the_mapped_case_converts() {
    assert!(matches!(FlatMapped::lift(Flat::A), Ok(FlatMapped::A)));
    assert_eq!(FlatMapped::lift(Flat::B).unwrap_err(), Flat::B);
    assert_eq!(FlatMapped::lift(Flat::C).unwrap_err(), Flat::C);
}

#[test]
fn a_listed_case_becomes_an_invariant_fault_carrying_the_original() {
    let result: Result<(), Fail<FlatMapped, lanes!(Fatal)>> = Err(Flat::C).lift();
    let Fail::Fatal(fatal) = result.unwrap_err() else {
        panic!("a listed case must become fatal")
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);
    assert_eq!(
        fatal.source().unwrap().downcast_ref::<Flat>(),
        Some(&Flat::C)
    );
}

#[derive(Debug, PartialEq)]
enum Inner {
    X,
    Y,
}

#[derive(Debug, PartialEq, errlanes::Rejection)]
enum Nested {
    #[rejection(code = "NESTED_A")]
    A,
    #[rejection(code = "NESTED_B")]
    B(Inner),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Nested, fatal(B))]
enum CoarseNested {
    #[lift(Nested::B(Inner::X))]
    #[rejection(code = "COARSE_X")]
    X,
    #[lift(Nested::A)]
    A,
}

#[test]
fn a_coarse_entry_catches_every_other_nested_case() {
    assert!(matches!(
        CoarseNested::lift(Nested::B(Inner::X)),
        Ok(CoarseNested::X)
    ));
    assert!(matches!(CoarseNested::lift(Nested::A), Ok(CoarseNested::A)));
    assert_eq!(
        CoarseNested::lift(Nested::B(Inner::Y)).unwrap_err(),
        Nested::B(Inner::Y)
    );
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Nested, fatal(B(Inner::X)))]
enum ExactNested {
    #[lift(Nested::B(Inner::Y))]
    #[rejection(code = "EXACT_Y")]
    Y,
    #[lift(Nested::A)]
    A,
}

#[test]
fn an_exact_nested_entry_lists_one_leaf() {
    assert!(matches!(
        ExactNested::lift(Nested::B(Inner::Y)),
        Ok(ExactNested::Y)
    ));
    assert_eq!(
        ExactNested::lift(Nested::B(Inner::X)).unwrap_err(),
        Nested::B(Inner::X)
    );
}

fn is_even(amount: &u8) -> bool {
    amount.is_multiple_of(2)
}

#[derive(Debug, PartialEq, errlanes::Rejection)]
enum Guarded {
    #[rejection(code = "GUARDED_A")]
    A(u8),
    #[rejection(code = "GUARDED_B")]
    B,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Guarded, fatal(A, B))]
enum GuardedMapped {
    #[lift(Guarded::A, when = is_even)]
    #[rejection(code = "EVEN")]
    Even(u8),
}

#[test]
fn a_listed_case_covers_the_guard_miss_without_warnings() {
    assert!(matches!(
        GuardedMapped::lift(Guarded::A(2)),
        Ok(GuardedMapped::Even(2))
    ));
    assert_eq!(
        GuardedMapped::lift(Guarded::A(3)).unwrap_err(),
        Guarded::A(3)
    );
    assert_eq!(GuardedMapped::lift(Guarded::B).unwrap_err(), Guarded::B);
}

#[derive(Debug, PartialEq, errlanes::Rejection)]
enum Wide {
    #[rejection(code = "WIDE_A")]
    A { id: u8 },
    #[rejection(code = "WIDE_B")]
    B { reason: String },
    #[rejection(code = "WIDE_C")]
    C(u8, u8),
}

#[derive(Debug, PartialEq, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "WIDE_A_LIFTED")]
#[lift(Wide, variant = A, fatal(B, C))]
struct WideA {
    id: u8,
}

#[test]
fn a_struct_registration_lists_the_other_cases_of_any_shape() {
    assert_eq!(WideA::lift(Wide::A { id: 4 }).unwrap(), WideA { id: 4 });
    assert_eq!(
        WideA::lift(Wide::B { reason: "x".into() }).unwrap_err(),
        Wide::B { reason: "x".into() }
    );
    assert_eq!(WideA::lift(Wide::C(1, 2)).unwrap_err(), Wide::C(1, 2));
}
