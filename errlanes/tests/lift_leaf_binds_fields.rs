//! A case leaf may bind named fields: `S::V { a, .. }`. A plain arm then
//! builds the destination from them, converting each through `Into`.

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

#[derive(Debug)]
enum Validate {
    AccountLocked { account_id: u64, reason: String },
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
#[rejection(code = "HOLDING_ACCOUNT_LOCKED")]
#[error("account {account_id} is locked")]
struct AccountLocked {
    account_id: u64,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Posting, unhandled = fatal)]
enum Holding {
    #[lift(Posting::Validate(Validate::AccountLocked { account_id, .. }))]
    #[rejection(delegate, from)]
    #[error("{0}")]
    AccountLocked(AccountLocked),
}

#[test]
fn a_nested_named_field_case_builds_the_leaf_from_the_bound_field() {
    let lifted = Holding::lift(Posting::Validate(Validate::AccountLocked {
        account_id: 42,
        reason: "frozen".into(),
    }));
    let Ok(Holding::AccountLocked(leaf)) = lifted else {
        panic!("expected the locked leaf");
    };
    assert_eq!(leaf.account_id, 42);
    assert_eq!(
        code(&Holding::AccountLocked(leaf)),
        "HOLDING_ACCOUNT_LOCKED"
    );
}

#[test]
fn the_built_leafs_code_reaches_the_family_catalogue() {
    assert_eq!(catalogue::<Holding>(), ["HOLDING_ACCOUNT_LOCKED"]);
}

#[test]
fn a_sibling_case_stays_unmapped() {
    assert!(matches!(
        Holding::lift(Posting::Validate(Validate::Other)),
        Err(Posting::Validate(Validate::Other))
    ));
}

#[derive(Debug, errlanes::Rejection)]
enum Source {
    #[rejection(code = "SRC_ONE")]
    One { a: u8 },
    #[rejection(code = "SRC_TWO")]
    Two { a: u8, b: String },
    #[rejection(code = "SRC_WIDE")]
    Wide { a: u8 },
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "ONE_LEAF")]
#[error("one")]
struct OneLeaf {
    a: u8,
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "TWO_LEAF")]
#[error("two")]
struct TwoLeaf {
    a: u8,
    b: String,
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "WIDE_LEAF")]
#[error("wide")]
struct WideLeaf {
    a: u32,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
enum Built {
    #[lift(Source::One { a })]
    #[rejection(delegate, from)]
    #[error("{0}")]
    One(OneLeaf),
    #[lift(Source::Two { a, b })]
    #[rejection(delegate, from)]
    #[error("{0}")]
    Two(TwoLeaf),
    #[lift(Source::Wide { a, .. })]
    #[rejection(delegate, from)]
    #[error("{0}")]
    Wide(WideLeaf),
}

#[test]
fn a_bound_field_without_rest_covers_the_whole_source_variant() {
    let Ok(Built::One(leaf)) = Built::lift(Source::One { a: 5 }) else {
        panic!("expected One");
    };
    assert_eq!(leaf.a, 5);
}

#[test]
fn two_bound_fields_build_one_leaf() {
    let Ok(Built::Two(leaf)) = Built::lift(Source::Two {
        a: 1,
        b: "x".into(),
    }) else {
        panic!("expected Two");
    };
    assert_eq!((leaf.a, leaf.b.as_str()), (1, "x"));
}

#[test]
fn each_bound_field_converts_through_into() {
    let Ok(Built::Wide(leaf)) = Built::lift(Source::Wide { a: 200 }) else {
        panic!("expected Wide");
    };
    assert_eq!(leaf.a, 200u32);
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
enum Direct {
    #[lift(Source::Two { a, b })]
    #[rejection(code = "DIRECT_TWO")]
    #[error("two")]
    Two { a: u32, b: String },
}

#[test]
fn destination_fields_named_like_the_bound_set_are_assigned_directly() {
    let Ok(Direct::Two { a, b }) = Direct::lift(Source::Two {
        a: 9,
        b: "y".into(),
    }) else {
        panic!("expected Two");
    };
    assert_eq!((a, b.as_str()), (9, "y"));
}

#[derive(Debug)]
struct Wrapped(u64);

impl From<u64> for Wrapped {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Posting, unhandled = fatal)]
enum Precedence {
    #[lift(Posting::Validate(Validate::AccountLocked { account_id, .. }))]
    #[rejection(code = "PRECEDENCE_LOCKED")]
    #[error("locked")]
    Locked { account_id: Wrapped },
}

#[test]
fn matching_names_assign_directly_and_never_build_a_leaf_literal() {
    let lifted = Precedence::lift(Posting::Validate(Validate::AccountLocked {
        account_id: 7,
        reason: String::new(),
    }));
    let Ok(Precedence::Locked { account_id }) = lifted else {
        panic!("expected Locked");
    };
    assert_eq!(account_id.0, 7);
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Posting, unhandled = fatal)]
enum NamedHolder {
    #[lift(Posting::Validate(Validate::AccountLocked { account_id, .. }))]
    #[rejection(code = "NAMED_HOLDER_LOCKED")]
    #[error("locked")]
    Locked { leaf: AccountLocked },
}

#[test]
fn one_named_destination_field_holds_the_built_leaf() {
    let lifted = NamedHolder::lift(Posting::Validate(Validate::AccountLocked {
        account_id: 3,
        reason: String::new(),
    }));
    let Ok(NamedHolder::Locked { leaf }) = lifted else {
        panic!("expected Locked");
    };
    assert_eq!(leaf.account_id, 3);
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
#[lift(Source::One { a }, to = Routed)]
enum Targeted {
    #[rejection(delegate, from)]
    #[error("{0}")]
    Routed(OneLeaf),
}

#[test]
fn bound_fields_combine_with_to() {
    let Ok(Targeted::Routed(leaf)) = Targeted::lift(Source::One { a: 8 }) else {
        panic!("expected Routed");
    };
    assert_eq!(leaf.a, 8);
}
