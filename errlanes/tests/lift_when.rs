use errlanes::{Fail, FatalKind, Level, Lift, Rejection, ResultExt, lanes};
use std::error::Error;

#[derive(Debug, PartialEq)]
struct Limit {
    id: &'static str,
    amount: u64,
}

fn is_overdraft(limit: &Limit) -> bool {
    limit.id == "overdraft"
}

fn is_floor(limit: &Limit) -> bool {
    limit.id == "floor"
}

fn is_big(limit: &Limit) -> bool {
    limit.amount > 100
}

#[derive(Debug, PartialEq, errlanes::Rejection)]
enum Source {
    #[rejection(code = "LIMIT_EXCEEDED", level = "warn")]
    Exceeded(Limit),
    #[rejection(code = "LOCKED")]
    Locked,
}

#[derive(Debug, PartialEq)]
struct Overdraft(Limit);
impl From<Limit> for Overdraft {
    fn from(limit: Limit) -> Self {
        Self(limit)
    }
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
enum Routed {
    #[lift(Source::Exceeded, when = is_overdraft, into)]
    #[rejection(code = "OVERDRAFT")]
    Overdraft(Overdraft),
    #[lift(Source::Exceeded, when = is_floor)]
    Floor(Limit),
    #[lift(Source::Exceeded)]
    OtherLimit(Limit),
}

#[test]
fn each_predicate_routes_to_its_own_destination_and_the_rest_reach_the_unguarded_arm() {
    assert!(matches!(
        Routed::lift(Source::Exceeded(Limit {
            id: "overdraft",
            amount: 1
        })),
        Ok(Routed::Overdraft(Overdraft(Limit {
            id: "overdraft",
            ..
        })))
    ));
    assert!(matches!(
        Routed::lift(Source::Exceeded(Limit {
            id: "floor",
            amount: 1
        })),
        Ok(Routed::Floor(Limit { id: "floor", .. }))
    ));
    assert!(matches!(
        Routed::lift(Source::Exceeded(Limit {
            id: "other",
            amount: 1
        })),
        Ok(Routed::OtherLimit(Limit { id: "other", .. }))
    ));
}

#[test]
fn a_different_source_variant_is_unmapped_and_becomes_fatal_with_the_original_as_source() {
    assert_eq!(Routed::lift(Source::Locked).unwrap_err(), Source::Locked);
    let result: Result<(), Fail<Routed, lanes!(Fatal)>> = Err(Source::Locked).lift();
    let Fail::Fatal(fatal) = result.unwrap_err() else {
        panic!("an unmapped source must become fatal")
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);
    assert_eq!(
        fatal.source().unwrap().downcast_ref::<Source>(),
        Some(&Source::Locked)
    );
}

#[test]
fn a_guarded_plain_arm_forwards_the_source_code_and_level() {
    let floor = Routed::lift(Source::Exceeded(Limit {
        id: "floor",
        amount: 1,
    }))
    .unwrap();
    assert_eq!(Into::<&'static str>::into(floor.code()), "LIMIT_EXCEEDED");
    assert_eq!(floor.level(), Level::Warn);
    let overdraft = Routed::lift(Source::Exceeded(Limit {
        id: "overdraft",
        amount: 1,
    }))
    .unwrap();
    assert_eq!(Into::<&'static str>::into(overdraft.code()), "OVERDRAFT");
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
enum GuardsOnly {
    #[lift(Source::Exceeded, when = is_overdraft)]
    Overdraft(Limit),
}

#[test]
fn a_predicate_miss_returns_the_original_and_becomes_fatal() {
    let miss = Source::Exceeded(Limit {
        id: "floor",
        amount: 7,
    });
    assert_eq!(
        GuardsOnly::lift(miss).unwrap_err(),
        Source::Exceeded(Limit {
            id: "floor",
            amount: 7
        })
    );
    let result: Result<(), Fail<GuardsOnly, lanes!(Fatal)>> = Err(Source::Exceeded(Limit {
        id: "floor",
        amount: 7,
    }))
    .lift();
    let Fail::Fatal(fatal) = result.unwrap_err() else {
        panic!("a predicate miss must become fatal")
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);
    assert_eq!(
        fatal.source().unwrap().downcast_ref::<Source>(),
        Some(&Source::Exceeded(Limit {
            id: "floor",
            amount: 7
        }))
    );
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source)]
enum Strict {
    #[lift(Source::Exceeded, when = is_overdraft)]
    Overdraft(Limit),
    #[lift(Source::Exceeded)]
    Exceeded(Limit),
    #[lift(Source::Locked)]
    Locked,
}

#[test]
fn a_strict_lift_with_a_catch_all_emits_from_and_works_with_the_question_mark() {
    fn step(source: Source) -> Result<(), Fail<Strict, lanes!()>> {
        Err(source)?;
        Ok(())
    }
    let Fail::Rejected(error) = step(Source::Exceeded(Limit {
        id: "overdraft",
        amount: 1,
    }))
    .unwrap_err();
    assert!(matches!(error, Strict::Overdraft(_)));
    let Fail::Rejected(error) = step(Source::Exceeded(Limit { id: "x", amount: 1 })).unwrap_err();
    assert!(matches!(error, Strict::Exceeded(_)));
    let Fail::Rejected(error) = step(Source::Locked).unwrap_err();
    assert!(matches!(error, Strict::Locked));
}

#[derive(Debug, PartialEq)]
struct Amount(u64);
impl From<Limit> for Amount {
    fn from(limit: Limit) -> Self {
        Self(limit.amount)
    }
}
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
enum Forms {
    #[lift(Source::Exceeded, when = is_overdraft, into)]
    #[rejection(code = "INTO")]
    Into(Amount),
    #[lift(Source::Exceeded, when = is_floor, field = amount)]
    #[rejection(code = "FIELD")]
    Field(u64),
    #[lift(Source::Exceeded, when = is_big, with = whole_source)]
    #[rejection(code = "WITH")]
    With(&'static str),
}

fn whole_source(source: Source) -> Forms {
    match source {
        Source::Exceeded(limit) => Forms::With(limit.id),
        Source::Locked => unreachable!(),
    }
}

#[test]
fn into_field_and_with_arms_accept_a_guard() {
    assert!(matches!(
        Forms::lift(Source::Exceeded(Limit {
            id: "overdraft",
            amount: 5
        })),
        Ok(Forms::Into(Amount(5)))
    ));
    assert!(matches!(
        Forms::lift(Source::Exceeded(Limit {
            id: "floor",
            amount: 6
        })),
        Ok(Forms::Field(6))
    ));
    assert!(matches!(
        Forms::lift(Source::Exceeded(Limit {
            id: "x",
            amount: 500
        })),
        Ok(Forms::With("x"))
    ));
    assert_eq!(
        Forms::lift(Source::Exceeded(Limit { id: "x", amount: 1 })).unwrap_err(),
        Source::Exceeded(Limit { id: "x", amount: 1 })
    );
}
