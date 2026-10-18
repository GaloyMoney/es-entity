use errlanes::{Fail, Lift, Rejection, lanes};

#[derive(Debug)]
struct LimitExceeded {
    limit: u64,
}

#[derive(Debug, errlanes::Rejection)]
enum Limits {
    #[rejection(code = "OVER_LIMIT")]
    Over(LimitExceeded),
    #[rejection(code = "SMALL")]
    Small(LimitExceeded),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "PAYMENT_EXCEEDS_OUTSTANDING")]
#[error("payment exceeds outstanding")]
#[lift(LimitExceeded)]
struct PaymentExceedsOutstanding;

#[test]
fn unit_struct_drops_the_value_and_keeps_its_own_code() {
    let leaf = PaymentExceedsOutstanding::from(LimitExceeded { limit: 3 });
    assert_eq!(
        Into::<&'static str>::into(leaf.code()),
        "PAYMENT_EXCEEDS_OUTSTANDING"
    );
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Limits)]
enum Payment {
    #[lift(Limits::Over, when = is_big, into)]
    #[rejection(code = "PAYMENT_OVER")]
    Over(PaymentExceedsOutstanding),
    #[lift(Limits::Over)]
    #[rejection(code = "PLAIN_OVER")]
    Plain(LimitExceeded),
    #[lift(Limits::Small)]
    Small(LimitExceeded),
}

fn is_big(limit: &LimitExceeded) -> bool {
    limit.limit > 100
}

#[test]
fn guarded_into_arm_converts_through_the_derived_unit_from() {
    let big = Payment::lift(Limits::Over(LimitExceeded { limit: 500 })).unwrap();
    assert!(matches!(big, Payment::Over(_)));
    let small = Payment::lift(Limits::Over(LimitExceeded { limit: 5 })).unwrap();
    assert!(matches!(small, Payment::Plain(_)));
}

#[test]
fn guarded_arms_leave_other_source_variants_to_their_own_arm() {
    let small = Payment::lift(Limits::Small(LimitExceeded { limit: 1 })).unwrap();
    assert!(matches!(small, Payment::Small(_)));
}

#[derive(Debug, errlanes::Rejection)]
enum Family {
    #[rejection(delegate, from)]
    #[error("{0}")]
    Exceeds(PaymentExceedsOutstanding),
}

#[test]
fn unit_struct_is_a_question_mark_source_into_a_family() {
    fn run() -> Result<(), Fail<Family, lanes!()>> {
        Err(PaymentExceedsOutstanding::from(LimitExceeded { limit: 1 }))?
    }
    let Fail::Rejected(family) = run().unwrap_err();
    assert!(matches!(family, Family::Exceeds(_)));
    assert_eq!(
        Into::<&'static str>::into(family.code()),
        "PAYMENT_EXCEEDS_OUTSTANDING"
    );
}
