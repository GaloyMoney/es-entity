//! A case leaf written `{ .. }` binds nothing: a plain arm then drops the
//! payload into a unit variant or a variant holding one unit leaf.

#![allow(dead_code)]

use errlanes::{Lift as _, Rejection, RejectionCode};

fn code<R: Rejection>(rejection: &R) -> &'static str {
    rejection.code().into()
}

fn catalogue<R: Rejection>() -> Vec<&'static str> {
    <R::Code as RejectionCode>::CODES
        .iter()
        .map(|entry| entry.code)
        .collect()
}

#[derive(Debug, errlanes::Rejection)]
enum Source {
    #[rejection(code = "SRC_TUPLE")]
    Tuple(u64),
    #[rejection(code = "SRC_NAMED")]
    Named { id: u64, reason: String },
    #[rejection(code = "SRC_OTHER")]
    Other,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
enum UnitDestinations {
    #[lift(Source::Tuple { .. })]
    #[rejection(code = "DEST_TUPLE")]
    Tuple,
    #[lift(Source::Named { .. })]
    #[rejection(code = "DEST_NAMED")]
    Named,
}

#[test]
fn a_unit_destination_drops_a_tuple_payload() {
    let lifted = UnitDestinations::lift(Source::Tuple(7));
    assert!(matches!(lifted, Ok(UnitDestinations::Tuple)));
    assert_eq!(code(&lifted.unwrap()), "DEST_TUPLE");
}

#[test]
fn a_unit_destination_drops_a_named_field_payload() {
    let lifted = UnitDestinations::lift(Source::Named {
        id: 1,
        reason: "gone".into(),
    });
    assert!(matches!(lifted, Ok(UnitDestinations::Named)));
}

#[test]
fn a_case_without_an_arm_stays_unmapped() {
    assert!(matches!(
        UnitDestinations::lift(Source::Other),
        Err(Source::Other)
    ));
}

#[derive(Debug)]
struct AccountId(u64);

#[derive(Debug)]
enum Validate {
    AccountLocked(AccountId),
    Other,
}

#[derive(Debug, errlanes::Rejection)]
enum Posting {
    #[rejection(code = "POSTING_VALIDATE")]
    Validate(Validate),
    #[rejection(code = "POSTING_OTHER")]
    Other,
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "SETTLEMENT_ACCOUNT_LOCKED")]
#[error("the settlement account is locked")]
struct SettlementAccountLocked;

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Posting, unhandled = fatal)]
enum Settlement {
    #[lift(Posting::Validate(Validate::AccountLocked { .. }))]
    #[rejection(delegate, from)]
    #[error("{0}")]
    AccountLocked(SettlementAccountLocked),
}

#[test]
fn a_nested_case_becomes_a_variant_holding_a_unit_leaf() {
    let lifted = Settlement::lift(Posting::Validate(Validate::AccountLocked(AccountId(9))));
    assert!(matches!(lifted, Ok(Settlement::AccountLocked(_))));
    assert_eq!(code(&lifted.unwrap()), "SETTLEMENT_ACCOUNT_LOCKED");
}

#[test]
fn a_sibling_of_a_nested_case_stays_unmapped() {
    assert!(matches!(
        Settlement::lift(Posting::Validate(Validate::Other)),
        Err(Posting::Validate(Validate::Other))
    ));
}

#[test]
fn the_unit_leafs_code_reaches_the_family_catalogue() {
    assert_eq!(catalogue::<Settlement>(), ["SETTLEMENT_ACCOUNT_LOCKED"]);
}

#[derive(Debug, errlanes::Rejection)]
enum Ensure {
    #[rejection(code = "KEYS_HELD_OUTSIDE_SCOPE")]
    KeysHeldOutsideScope { ids: Vec<u64> },
    #[rejection(code = "ENSURE_OTHER")]
    Other,
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "DEPOSIT_REFERENCE_TAKEN")]
#[error("the deposit reference is taken")]
struct DepositReferenceTaken;

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Ensure, unhandled = fatal)]
enum Record {
    #[lift(Ensure::KeysHeldOutsideScope { .. })]
    #[rejection(delegate, from)]
    #[error("{0}")]
    ReferenceTaken(DepositReferenceTaken),
}

#[test]
fn a_named_field_source_drops_into_a_variant_holding_a_unit_leaf() {
    let lifted = Record::lift(Ensure::KeysHeldOutsideScope { ids: vec![1, 2] });
    assert!(matches!(lifted, Ok(Record::ReferenceTaken(_))));
    assert_eq!(code(&lifted.unwrap()), "DEPOSIT_REFERENCE_TAKEN");
}

#[derive(Debug, errlanes::Rejection)]
enum Single {
    #[rejection(code = "SINGLE_TUPLE")]
    Tuple(u64),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Single)]
enum NamedHolder {
    #[lift(Single::Tuple { .. })]
    #[rejection(code = "NAMED_HOLDER_LOCKED")]
    #[error("{leaf}")]
    Locked { leaf: DepositReferenceTaken },
}

#[test]
fn a_named_destination_field_holds_the_unit_leaf() {
    let lifted = NamedHolder::from(Single::Tuple(3));
    assert!(matches!(lifted, NamedHolder::Locked { .. }));
    assert_eq!(code(&lifted), "NAMED_HOLDER_LOCKED");
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
#[lift(Source::Tuple { .. }, to = Dropped)]
enum Target {
    #[rejection(code = "TARGET_DROPPED")]
    Dropped,
    #[lift(Source::Named)]
    #[rejection(code = "TARGET_NAMED")]
    Named { id: u64, reason: String },
}

#[test]
fn a_braced_leaf_composes_with_to() {
    assert!(matches!(
        Target::lift(Source::Tuple(1)),
        Ok(Target::Dropped)
    ));
    assert!(matches!(
        Target::lift(Source::Named {
            id: 4,
            reason: "x".into()
        }),
        Ok(Target::Named { id: 4, .. })
    ));
}
