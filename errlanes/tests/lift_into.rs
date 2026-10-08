use errlanes::{Fail, FatalKind, Level, Lift, Rejection, ResultExt, lanes};
use std::error::Error;

#[derive(Debug, errlanes::Rejection)]
enum Prepare {
    #[error("missing {0}")]
    #[rejection(code = "MISSING", level = "warn")]
    Missing(String),
}

#[errlanes::compose(Prepare as Posting)]
#[derive(Debug)]
enum BatchPrepare {
    Duplicate,
}

#[derive(Debug, errlanes::Rejection)]
enum Posting {
    #[error("{0}")]
    #[rejection(delegate, from)]
    Prepare(Prepare),
    #[rejection(code = "LOCKED")]
    Validate {
        account: String,
    },
    Apply,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Posting)]
enum BatchPosting {
    #[lift(Posting::Prepare, into)]
    #[error("{0}")]
    #[rejection(delegate, from)]
    Prepare(BatchPrepare),
    #[lift(Posting::Validate)]
    Validate { account: String },
    #[lift(Posting::Apply)]
    Apply,
}

#[test]
fn strict_lift_converts_nested_payload_and_delegates_diagnostics() {
    let result: Result<(), Fail<BatchPosting, lanes!()>> =
        Err::<(), _>(Posting::Prepare(Prepare::Missing("amount".into()))).map_err(Into::into);
    let Fail::Rejected(error) = result.unwrap_err();
    assert!(matches!(
        &error,
        BatchPosting::Prepare(BatchPrepare::PostingMissing(name)) if name == "amount"
    ));
    assert_eq!(Into::<&'static str>::into(error.code()), "MISSING");
    assert_eq!(error.level(), Level::Warn);
    assert_eq!(error.to_string(), "missing amount");
    assert!(matches!(
        error.source().unwrap().downcast_ref::<BatchPrepare>(),
        Some(BatchPrepare::PostingMissing(name)) if name == "amount"
    ));
    assert!(matches!(
        BatchPosting::from(Posting::Validate { account: "a".into() }),
        BatchPosting::Validate { account } if account == "a"
    ));
    assert!(matches!(
        BatchPosting::from(Posting::Apply),
        BatchPosting::Apply
    ));
    // The variant's own `from` remains available alongside the derived lift.
    assert!(matches!(
        BatchPosting::from(BatchPrepare::Duplicate),
        BatchPosting::Prepare(BatchPrepare::Duplicate)
    ));
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Posting, unhandled = fatal)]
enum PrepareOnly {
    #[lift(Posting::Prepare, into)]
    #[rejection(code = "PREPARE_ONLY")]
    Prepare {
        #[source]
        source: BatchPrepare,
    },
}

#[test]
fn partial_lift_converts_into_a_named_destination_and_preserves_unmapped_source() {
    assert!(matches!(
        PrepareOnly::lift(Posting::Prepare(Prepare::Missing("amount".into()))).unwrap(),
        PrepareOnly::Prepare { source: BatchPrepare::PostingMissing(name) } if name == "amount"
    ));
    assert!(matches!(
        PrepareOnly::lift(Posting::Apply).unwrap_err(),
        Posting::Apply
    ));
    let result: Result<(), Fail<PrepareOnly, lanes!(Fatal)>> = Err(Posting::Apply).lift();
    let Fail::Fatal(fatal) = result.unwrap_err() else {
        panic!("unmapped phase must become fatal")
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);
    assert!(matches!(
        fatal.source().unwrap().downcast_ref::<Posting>(),
        Some(Posting::Apply)
    ));
}

#[derive(Debug, errlanes::Rejection)]
enum Small {
    Number(u8),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Small)]
enum Large {
    #[lift(Small::Number, into)]
    #[rejection(code = "LARGE_NUMBER", level = "error")]
    Number(u64),
}

#[test]
fn converted_payload_can_use_explicit_destination_metadata() {
    let error = Large::from(Small::Number(42));
    assert!(matches!(error, Large::Number(42)));
    assert_eq!(Into::<&'static str>::into(error.code()), "LARGE_NUMBER");
    assert_eq!(error.level(), Level::Error);
}

// Lift remains usable without Rejection, and calls the Into trait explicitly
// even when the payload defines an unrelated inherent method named `into`.
struct Payload(String);
impl Payload {
    #[allow(dead_code)]
    fn into(self) -> bool {
        false
    }
}
impl From<Payload> for String {
    fn from(value: Payload) -> Self {
        value.0
    }
}
enum Source {
    Value(Payload),
}
#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value, into)]
    Value(String),
}

#[test]
fn lift_alone_uses_trait_conversion_and_moves_the_payload() {
    let Destination::Value(value) = Destination::from(Source::Value(Payload("owned".into())));
    assert_eq!(value, "owned");
}
