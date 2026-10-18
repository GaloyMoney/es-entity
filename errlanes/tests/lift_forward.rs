use errlanes::{Fail, FatalKind, Level, Lift as _, Rejection, ResultExt, lanes};

#[derive(Debug, PartialEq, errlanes::Rejection)]
enum Transition {
    #[rejection(code = "ILLEGAL_TRANSITION", level = "warn")]
    Illegal(u8),
    #[rejection(code = "STALE_TRANSITION")]
    Stale,
}

#[derive(Debug, errlanes::Rejection)]
enum Update {
    #[rejection(delegate)]
    Transition(Transition),
    #[rejection(code = "INVALID_UPDATE")]
    Invalid(String),
}

#[errlanes::compose(Transition)]
#[derive(Debug)]
#[lift(Update, unhandled = fatal)]
#[lift(Update::Transition, into)]
enum UpdateFamily {}

#[test]
fn a_forwarding_arm_builds_self_from_the_payload() {
    let lifted = UpdateFamily::lift(Update::Transition(Transition::Illegal(3)));
    let Ok(UpdateFamily::Illegal(3)) = lifted else {
        panic!("expected the composed variant, got {lifted:?}");
    };
    assert_eq!(lifted.as_ref().unwrap().level(), Level::Warn);
    assert_eq!(
        Into::<&'static str>::into(lifted.unwrap().code()),
        "ILLEGAL_TRANSITION"
    );
}

#[test]
fn an_unmapped_case_stays_unmapped_and_becomes_fatal_through_lift() {
    let unmapped = UpdateFamily::lift(Update::Invalid("bad".into()));
    assert!(matches!(unmapped, Err(Update::Invalid(ref text)) if text == "bad"));

    let result: Result<(), Fail<UpdateFamily, lanes!(Fatal)>> =
        Err(Update::Invalid("bad".into())).lift();
    let Fail::Fatal(fatal) = result.unwrap_err() else {
        panic!("expected a fatal lane");
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);
}

fn is_stale(transition: &Transition) -> bool {
    matches!(transition, Transition::Stale)
}

#[errlanes::compose(Transition)]
#[derive(Debug)]
#[lift(Update, unhandled = fatal)]
#[lift(Update::Transition, when = is_stale, into)]
enum GuardedFamily {}

#[test]
fn a_guarded_forwarding_arm_only_forwards_what_the_guard_accepts() {
    assert!(matches!(
        GuardedFamily::lift(Update::Transition(Transition::Stale)),
        Ok(GuardedFamily::Stale)
    ));
    assert!(matches!(
        GuardedFamily::lift(Update::Transition(Transition::Illegal(1))),
        Err(Update::Transition(Transition::Illegal(1)))
    ));
}

fn from_update(update: Update) -> WithFamily {
    match update {
        Update::Transition(Transition::Illegal(n)) => WithFamily::Illegal(n),
        _ => WithFamily::Stale,
    }
}

#[errlanes::compose(Transition)]
#[derive(Debug)]
#[lift(Update, unhandled = fatal)]
#[lift(Update::Transition, with = from_update)]
enum WithFamily {}

#[test]
fn a_with_forwarding_arm_hands_the_mapper_the_source_case() {
    assert!(matches!(
        WithFamily::lift(Update::Transition(Transition::Illegal(9))),
        Ok(WithFamily::Illegal(9))
    ));
    assert!(matches!(
        WithFamily::lift(Update::Transition(Transition::Stale)),
        Ok(WithFamily::Stale)
    ));
}

#[derive(Debug)]
enum Wrapper {
    Transition(Transition),
}

#[errlanes::compose(Transition)]
#[derive(Debug)]
#[lift(Wrapper)]
#[lift(Wrapper::Transition, into)]
enum StrictFamily {}

#[test]
fn a_forwarding_arm_on_a_strict_registration_emits_from() {
    assert!(matches!(
        StrictFamily::from(Wrapper::Transition(Transition::Illegal(4))),
        StrictFamily::Illegal(4)
    ));
}

#[allow(dead_code)]
#[derive(Debug, errlanes::Rejection)]
enum PersonalInfo {
    #[rejection(code = "DATE_OF_BIRTH_IN_FUTURE")]
    DateOfBirthInFuture,
    #[rejection(code = "NAME_REQUIRED")]
    NameRequired(String),
}

#[derive(Debug, errlanes::Rejection)]
enum Forwarded {
    #[rejection(code = "DOB")]
    Dob,
    #[rejection(code = "NAME")]
    Name(String),
    #[rejection(code = "NOT_FOUND")]
    NotFound,
}

#[errlanes::compose(PersonalInfo)]
#[derive(Debug)]
#[lift(Forwarded, unhandled = fatal)]
#[lift(Forwarded::Dob, to = DateOfBirthInFuture)]
#[lift(Forwarded::Name, to = NameRequired)]
enum Record {}

#[test]
fn to_lands_a_case_in_a_composed_in_variant() {
    let dob = Record::lift(Forwarded::Dob).unwrap();
    assert!(matches!(dob, Record::DateOfBirthInFuture));
    assert_eq!(
        Into::<&'static str>::into(dob.code()),
        "DATE_OF_BIRTH_IN_FUTURE"
    );
    assert!(matches!(
        Record::lift(Forwarded::Name("x".into())),
        Ok(Record::NameRequired(name)) if name == "x"
    ));
    assert!(matches!(
        Record::lift(Forwarded::NotFound),
        Err(Forwarded::NotFound)
    ));
}

fn is_empty_name(name: &String) -> bool {
    name.is_empty()
}

#[errlanes::compose(PersonalInfo)]
#[derive(Debug)]
#[lift(Forwarded, unhandled = fatal)]
#[lift(Forwarded::Name, when = is_empty_name, to = NameRequired)]
enum Guarded {
    #[lift(Forwarded::Name)]
    Named(String),
}

#[test]
fn a_guarded_enum_level_arm_precedes_a_variant_level_catch_all() {
    assert!(matches!(
        Guarded::lift(Forwarded::Name(String::new())),
        Ok(Guarded::NameRequired(name)) if name.is_empty()
    ));
    assert!(matches!(
        Guarded::lift(Forwarded::Name("x".into())),
        Ok(Guarded::Named(name)) if name == "x"
    ));
}
