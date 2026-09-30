use errlanes::{Fail, Level, LiftResult, Rejection, WidenResult, lanes};

use std::error::Error;

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Enforcement {
    #[error("limit {0}")]
    #[rejection(code = "ENFORCEMENT", level = "warn")]
    Limit(u64),
    #[error("disabled")]
    Disabled,
    #[error("range {min}..{max}")]
    Range { min: u64, max: u64 },
}

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Constraint {
    #[error("code {0}")]
    Code(String),
    #[error("primary key conflict")]
    Pkey,
}

#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
#[lift(Constraint, unhandled = fatal)]
enum Operation {
    #[compose(flatten)]
    Velocity(Enforcement),
    #[lift(Constraint::Code)]
    #[rejection(code = "ACCOUNT_CODE_ALREADY_EXISTS")]
    #[error("account code {0} already exists")]
    CodeAlreadyExists(String),
    #[error("local")]
    Local,
}

#[test]
fn whole_family_and_partial_mapping_keep_their_own_modes() {
    let source: Result<(), Fail<Enforcement, lanes!(Fatal)>> =
        Err(Fail::Rejected(Enforcement::Limit(42)));
    let result: Result<(), Fail<Operation, lanes!(Transient, Fatal)>> = source.widen();
    let mapped = result.unwrap_err().rejected().unwrap();
    assert!(matches!(mapped, Operation::VelocityLimit(42)));
    assert_eq!(Into::<&'static str>::into(mapped.code()), "ENFORCEMENT");
    assert_eq!(mapped.level(), Level::Warn);
    assert_eq!(mapped.to_string(), "limit 42");
    assert!(matches!(
        Operation::from(Enforcement::Disabled),
        Operation::VelocityDisabled
    ));
    assert!(matches!(
        Operation::from(Enforcement::Range { min: 1, max: 3 }),
        Operation::VelocityRange { min: 1, max: 3 }
    ));

    let accepted: Result<(), Fail<Constraint, lanes!(Transient, Fatal)>> =
        Err(Fail::Rejected(Constraint::Code("USD".into())));
    let result: Result<(), Fail<Operation, lanes!(Transient, Fatal)>> = accepted.lift();
    let mapped = result.unwrap_err().rejected().unwrap();
    assert!(matches!(&mapped, Operation::CodeAlreadyExists(value) if value == "USD"));
    assert_eq!(
        Into::<&'static str>::into(mapped.code()),
        "ACCOUNT_CODE_ALREADY_EXISTS"
    );
    assert_eq!(mapped.level(), Level::Info);

    let unaccepted: Result<(), Fail<Constraint, lanes!(Fatal)>> =
        Err(Fail::Rejected(Constraint::Pkey));
    let result: Result<(), Fail<Operation, lanes!(Fatal)>> = unaccepted.lift();
    let error = result.unwrap_err();
    assert!(matches!(error, Fail::Fatal(_)));
    let mut source: &dyn Error = &error;
    while source.downcast_ref::<Constraint>().is_none() {
        source = source
            .source()
            .expect("original constraint in fatal source chain");
    }
    assert!(matches!(
        source.downcast_ref::<Constraint>(),
        Some(Constraint::Pkey)
    ));
    assert_eq!(Operation::Local.to_string(), "local");
}

// Explicit strict lifts preserve custom or previously unprefixed names.
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
#[lift(Enforcement)]
enum CustomNames {
    #[compose(flatten)]
    Repo(Constraint),
    #[lift(Enforcement::Limit)]
    #[error("custom limit {0}")]
    Limit(u64),
    #[lift(Enforcement::Disabled)]
    #[error("unavailable")]
    Unavailable,
    #[lift(Enforcement::Range)]
    #[error("custom range {min}..{max}")]
    Range { min: u64, max: u64 },
}

#[test]
fn whole_family_and_explicit_strict_mapping_preserve_custom_names() {
    let value = CustomNames::from(Enforcement::Limit(9));
    assert!(matches!(value, CustomNames::Limit(9)));
    assert_eq!(Into::<&'static str>::into(value.code()), "ENFORCEMENT");
    assert_eq!(value.level(), Level::Warn);
    assert!(matches!(
        CustomNames::from(Enforcement::Disabled),
        CustomNames::Unavailable
    ));
    assert!(matches!(
        CustomNames::from(Constraint::Pkey),
        CustomNames::RepoPkey
    ));
}

// compose supports explicit-only mappings and deduplicates ordinary derive lists,
// both qualified and imported spellings, without removing unrelated derives.
#[errlanes::compose]
#[derive(Debug, thiserror::Error, Rejection, errlanes::Lift, errlanes::Rejection, Clone)]
#[lift(Constraint)]
enum ExplicitOnly {
    #[lift(Constraint::Code)]
    #[error("code {0}")]
    Code(String),
    #[lift(Constraint::Pkey)]
    #[error("key")]
    Key,
}

#[test]
fn explicit_only_composition_generates_each_trait_once() {
    let value = ExplicitOnly::from(Constraint::Code("EUR".into()));
    assert!(matches!(value.clone(), ExplicitOnly::Code(code) if code == "EUR"));
    assert_eq!(value.level(), Level::Info);
    assert!(matches!(
        ExplicitOnly::from(Constraint::Pkey),
        ExplicitOnly::Key
    ));
}
