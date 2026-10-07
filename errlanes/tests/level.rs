//! `Laned::level` is the one public lane→level table. Deliberately not gated
//! on the `tracing` feature: `Level` is errlanes' own type.
use errlanes::{
    Denied, Fail, Fatal, FatalKind, Fault, Lane, Laned, Level, Transient, TransientKind,
};

#[derive(Debug, Clone, errlanes::Rejection)]
enum Levelled {
    #[rejection(code = "DEFAULTED")]
    Defaulted,
    #[rejection(code = "WARNED", level = "warn")]
    Warned,
}

#[derive(Debug, errlanes::Carrier)]
enum Carried {
    Rejected(Levelled),
    Denied(errlanes::Denied),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

#[test]
fn each_lane_has_its_default_level() {
    assert_eq!(Lane::Rejected.level(), Level::Info);
    assert_eq!(Lane::Denied.level(), Level::Warn);
    assert_eq!(Lane::Transient.level(), Level::Info);
    assert_eq!(Lane::Fatal.level(), Level::Error);
}

#[test]
fn a_built_in_reports_its_lane_default_level() {
    let denied: Fail<Levelled> = Denied::default().into();
    assert_eq!(Laned::level(&denied), Level::Warn);
    let transient: Fail<Levelled> = Transient::new(TransientKind::Deadlock).into();
    assert_eq!(Laned::level(&transient), Level::Info);
    let fatal: Fail<Levelled> = Fatal::new(FatalKind::Invariant).into();
    assert_eq!(Laned::level(&fatal), Level::Error);

    let fault: Fault = Fatal::new(FatalKind::Invariant).into();
    assert_eq!(Laned::level(&fault), Level::Error);
    let fault: Fault = Denied::default().into();
    assert_eq!(Laned::level(&fault), Level::Warn);
}

#[test]
fn a_rejection_overrides_the_lane_default_through_rejection_level() {
    let defaulted: Fail<Levelled> = Fail::Rejected(Levelled::Defaulted);
    assert_eq!(Laned::level(&defaulted), Lane::Rejected.level());

    let warned: Fail<Levelled> = Fail::Rejected(Levelled::Warned);
    assert_eq!(Laned::level(&warned), Level::Warn);
    assert_ne!(Laned::level(&warned), Lane::Rejected.level());
    assert_eq!(warned.lanes().level(), Level::Warn);
}

#[test]
fn a_carrier_reports_its_built_ins_level() {
    let pairs: Vec<(Carried, Fail<Levelled>)> = vec![
        (
            Carried::Rejected(Levelled::Warned),
            Fail::Rejected(Levelled::Warned),
        ),
        (
            Carried::Rejected(Levelled::Defaulted),
            Fail::Rejected(Levelled::Defaulted),
        ),
        (Carried::Denied(Denied::default()), Denied::default().into()),
        (
            Carried::Transient(Transient::new(TransientKind::Deadlock)),
            Transient::new(TransientKind::Deadlock).into(),
        ),
        (
            Carried::Fatal(Fatal::new(FatalKind::Config)),
            Fatal::new(FatalKind::Config).into(),
        ),
    ];
    for (carrier, builtin) in pairs {
        assert_eq!(Laned::level(&carrier), Laned::level(&builtin));
    }
}
