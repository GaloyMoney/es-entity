use errlanes::{
    Fail, Fatal, FatalKind, Fault, Lane, Rejection, ResultExt, Transient, TransientKind, lanes,
};
use std::{convert::Infallible, error::Error};

#[test]
fn fault_results_widen_at_question_mark_boundaries() {
    fn authorize_boundary(
        inner: Result<String, Fault<lanes!(Fatal)>>,
    ) -> Result<String, Fault<lanes!(Denied, Fatal)>> {
        let value = inner.widen()?;
        Ok(value)
    }

    assert_eq!(authorize_boundary(Ok("saved".into())).unwrap(), "saved");
    let source = Fatal::from_error(FatalKind::Config, std::io::Error::other("missing config"))
        .with_context("startup");
    let Fault::Fatal(error) = authorize_boundary(Err(source.into())).unwrap_err() else {
        panic!("expected fatal");
    };
    assert_eq!(error.kind, FatalKind::Config);
    assert_eq!(error.context.as_deref(), Some("startup"));
    assert_eq!(error.source().unwrap().to_string(), "missing config");
    assert!(error.source().unwrap().is::<std::io::Error>());
}

#[test]
fn widening_fault_results_preserves_transient_and_denied_payloads() {
    let transient = Transient::new(TransientKind::Deadlock)
        .with_context("retry transaction")
        .with_source(std::io::Error::other("deadlock"));
    let original_source = transient.source_arc().unwrap().clone();
    let result: Result<(), Fault<lanes!(Transient)>> = Err(transient.into());
    let widened: Result<(), Fault<lanes!(Transient, Fatal, Denied)>> = result.widen();
    let Fault::Transient(error) = widened.unwrap_err() else {
        panic!("expected transient");
    };
    assert_eq!(error.kind, TransientKind::Deadlock);
    assert_eq!(error.context.as_deref(), Some("retry transaction"));
    assert!(std::sync::Arc::ptr_eq(
        &original_source,
        error.source_arc().unwrap()
    ));

    let denied = errlanes::Denied {
        action: Some("write".into()),
        ..Default::default()
    };
    let result: Result<(), Fault<lanes!(Denied)>> = Err(denied.into());
    let widened: Result<(), Fault<lanes!(Denied, Fatal)>> = result.widen();
    let Fault::Denied(error) = widened.unwrap_err() else {
        panic!("expected denied");
    };
    assert_eq!(error.action.as_deref(), Some("write"));
}

#[derive(Debug, Clone, errlanes::Rejection)]
pub enum Child {
    Taken(String),
    Range { low: u32, high: u32 },
    Unit,
}
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child)]
pub enum Parent {
    #[lift(Child::Taken)]
    Taken(String),
    #[lift(Child::Range)]
    Range { low: u32, high: u32 },
    #[lift(Child::Unit)]
    Unit,
}
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, unhandled = fatal)]
pub enum Partial {
    #[lift(Child::Taken)]
    Taken(String),
}

#[test]
fn partial_lift_preserves_unmapped_source() {
    fn run() -> Result<(), Fail<Partial, lanes!(Fatal)>> {
        Err::<(), Fail<Child, lanes!()>>(Fail::Rejected(Child::Range { low: 2, high: 4 }))
            .widen()?;
        Ok(())
    }
    let Fail::Fatal(f) = run().unwrap_err() else {
        panic!("expected fatal")
    };
    assert_eq!(f.kind, FatalKind::Invariant);
    assert!(matches!(
        f.source().unwrap().downcast_ref::<Child>(),
        Some(Child::Range { low: 2, high: 4 })
    ));
}
#[test]
fn strict_lift_needs_no_fatal_lane() {
    fn run() -> Result<(), Fail<Parent, lanes!()>> {
        Err::<(), Fail<Child, lanes!()>>(Fail::Rejected(Child::Range { low: 2, high: 4 }))
            .widen()?;
        Ok(())
    }
    assert!(matches!(
        run(),
        Err(Fail::Rejected(Parent::Range { low: 2, high: 4 }))
    ));
    let source: Fail<Child, lanes!()> = Fail::Rejected(Child::Unit);
    let lifted: Fail<Parent, lanes!()> = source.widen();
    // Narrowing a profile with no transient lane is the identity on the value,
    // and its type is already a `Fail` -- there is nothing to convert back.
    let restored: Fail<Parent, lanes!()> = lifted.narrow_transient(1);
    assert!(matches!(restored, Fail::Rejected(Parent::Unit)));
}
#[test]
fn borrowed_narrowing_needs_no_uninhabited_arms() {
    // `lane()` and the borrowed accessors cover every borrowed inspection, so
    // no caller writes `match *never {}`.
    fn inspect(value: &Fail<Child, lanes!(Fatal)>) -> Lane {
        value.lane()
    }
    let value: Fail<Child, lanes!(Fatal)> = Fatal::invariant("broken").into();
    let narrowed = value.narrow_transient(1);
    assert_eq!(inspect(&narrowed), Lane::Fatal);
    assert!(narrowed.as_fatal().is_some());
    assert!(narrowed.as_rejected().is_none());
}
#[test]
fn widening_preserves_transient_details_and_boxed_marker() {
    let original: Fail<Child, lanes!(Transient)> =
        Transient::from_error(TransientKind::Deadlock, std::io::Error::other("source"))
            .with_context("operation")
            .into();
    let widened: Fail<Parent, lanes!(Fatal, Transient)> = original.widen();
    assert_eq!(errlanes::Lane::of(&widened), Some(Lane::Transient));
    let narrowed: Fail<Parent, lanes!(Fatal)> = widened.narrow_transient(3);
    let fatal = narrowed.as_fatal().expect("expected exhaustion");
    assert_eq!(fatal.kind, errlanes::FatalKind::Exhausted);
    let e = fatal
        .source()
        .and_then(|s| s.downcast_ref::<errlanes::Exhausted>())
        .expect("exhaustion is the fatal's source");
    assert_eq!(e.attempts, 3);
    assert_eq!(e.last.kind, TransientKind::Deadlock);
    assert!(e.last.source().unwrap().is::<std::io::Error>());
    let boxed: Box<dyn Error + Send + Sync> = Box::new(narrowed);
    assert_eq!(errlanes::Lane::of(boxed.as_ref()), Some(Lane::Fatal));
}
/// There is deliberately no `From<Box<dyn Error + Send + Sync>>` for a lane
/// carrier: it would silently discard whatever lane the box already holds.
/// A boxed error is classified ([`errlanes::Fault::classify`]) or demoted out
/// loud ([`Fatal::from_boxed`]); `Infallible` is the only free conversion.
#[test]
fn infallible_and_boxed_conversions_are_coherent() {
    #[allow(unreachable_code)]
    fn convert(x: Infallible) -> Fault<lanes!()> {
        x.into()
    }
    let _ = convert;

    let boxed: Box<dyn Error + Send + Sync> = Box::new(std::io::Error::other("source"));
    let fault: Fault = errlanes::Fault::classify(boxed.as_ref());
    assert_eq!(fault.lane(), Lane::Fatal);

    let boxed: Box<dyn Error + Send + Sync> = Box::new(std::io::Error::other("source"));
    let fault: Fault<lanes!(Fatal)> = Fatal::from_boxed(FatalKind::Dependency, boxed).into();
    assert_eq!(fault.lane(), Lane::Fatal);

    assert_eq!(Into::<&'static str>::into(Child::Unit.code()), "UNIT");
}

#[test]
fn retry_loop_respects_subset() {
    // `lanes!(Transient)` alone is no longer retryable: narrowing it would
    // have nowhere to put the exhaustion. Anything worth retrying can fail
    // permanently, so it must admit Fatal.
    fn retry(
        mut op: impl FnMut() -> Result<(), Fail<Child, lanes!(Transient, Fatal)>>,
    ) -> Result<(), Fail<Child, lanes!(Fatal)>> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            match op() {
                Ok(()) => return Ok(()),
                Err(failure) if failure.is_transient() && attempts < 3 => continue,
                Err(failure) => return Err(failure.narrow_transient(attempts)),
            }
        }
    }

    let mut attempts = 0;
    let result = retry(|| {
        attempts += 1;
        Err(Transient::new(TransientKind::Deadlock).into())
    });
    let fatal = result.as_ref().unwrap_err().as_fatal().unwrap();
    assert_eq!(fatal.kind, errlanes::FatalKind::Exhausted);
    assert_eq!(
        fatal
            .source()
            .and_then(|s| s.downcast_ref::<errlanes::Exhausted>())
            .unwrap()
            .attempts,
        3
    );
    assert_eq!(attempts, 3);

    fn fatal_only(
        mut op: impl FnMut() -> Result<(), Fail<Child, lanes!(Fatal)>>,
    ) -> Result<(), Fail<Child, lanes!(Fatal)>> {
        // A fatal-only profile has no transient lane to loop on: nothing
        // here ever calls `op` more than once.
        op()
    }
    let result = fatal_only(|| Err(Fatal::invariant("stored corruption").into()));
    assert!(matches!(result, Err(Fail::Fatal(_))));
}

#[test]
fn transparent_subset_wrapper_retains_lane_markers() {
    #[derive(Debug, thiserror::Error)]
    #[error(transparent)]
    struct Wrapper(Fail<Child, lanes!(Denied, Fatal)>);
    let boxed: Box<dyn Error + Send + Sync> = Box::new(Wrapper(errlanes::Denied::default().into()));
    assert_eq!(errlanes::Lane::of(boxed.as_ref()), Some(Lane::Denied));
    let boxed: Box<dyn Error + Send + Sync> = Box::new(Wrapper(Fatal::invariant("broken").into()));
    assert_eq!(errlanes::Lane::of(boxed.as_ref()), Some(Lane::Fatal));
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child)]
pub enum Renamed {
    #[lift(Child::Taken)]
    RenamedTaken(String),
    #[lift(Child::Range)]
    RenamedRange { low: u32, high: u32 },
    #[lift(Child::Unit)]
    RenamedUnit,
}
#[test]
fn explicit_strict_rename_preserves_leaf_code() {
    let parent: Renamed = Child::Taken("id".into()).into();
    assert_eq!(Into::<&'static str>::into(parent.code()), "TAKEN");
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, strict)]
pub enum Transformed {
    #[rejection(code = "CANONICAL")]
    #[lift(Child::Taken, with = transform)]
    #[lift(Child::Range, with = transform)]
    #[lift(Child::Unit, with = transform)]
    Canonical(String),
}
fn transform(source: Child) -> Transformed {
    Transformed::Canonical(source.to_string())
}
#[test]
fn mapper_consumes_all_supported_shapes() {
    for child in [
        Child::Taken("key".into()),
        Child::Range { low: 1, high: 2 },
        Child::Unit,
    ] {
        let expected = child.to_string();
        let result: Transformed = child.into();
        assert!(matches!(result, Transformed::Canonical(ref text) if *text == expected));
        assert_eq!(Into::<&'static str>::into(result.code()), "CANONICAL");
    }
}

#[derive(Debug, errlanes::Rejection)]
pub enum Other {
    #[rejection(code = "OTHER", level = "warn")]
    Unit,
}
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Other)]
#[lift(Child)]
pub enum Combined {
    #[rejection(code = "SHARED")]
    #[lift(Other::Unit)]
    #[lift(Child::Unit)]
    Unit,
    #[lift(Child::Taken)]
    Taken(String),
    #[lift(Child::Range)]
    Range { low: u32, high: u32 },
}
#[derive(Debug, errlanes::Rejection)]
pub enum Delegated {
    #[rejection(delegate, from)]
    Other(Other),
}
#[test]
fn multiple_sources_and_explicit_delegation() {
    assert!(matches!(Combined::from(Other::Unit), Combined::Unit));
    assert!(matches!(Combined::from(Child::Unit), Combined::Unit));
    let delegated = Delegated::from(Other::Unit);
    assert_eq!(delegated.level(), errlanes::Level::Warn);
    assert_eq!(Into::<&'static str>::into(delegated.code()), "OTHER");
}

// `#[lift(.., field = name)]` projects one field out of a source variant's
// payload, rather than forwarding the whole thing. A sibling source (not an
// extra variant on `Child`) keeps every existing `Child`-strict destination
// above exhaustive without needing a matching arm for it.
#[derive(Debug, Clone)]
pub struct ConflictPayload {
    pub attempted: u32,
    pub note: String,
}
#[derive(Debug, Clone, errlanes::Rejection)]
pub enum FieldSource {
    Conflict(ConflictPayload),
}

// A `field` projection, like a `with` mapper, cannot forward the source
// variant's code/level automatically: the destination only keeps the one
// named field, not the whole source payload, so there is no value of the
// right shape to hand the source's `RejectionMetadata`. An explicit
// `#[rejection(code = ..)]` is therefore required on every `field`-lift
// destination variant (enforced by `lift_field_requires_an_explicit_code`
// in `errlanes-derive/tests/v2/`).
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(FieldSource)]
pub enum Projected {
    #[rejection(code = "ATTEMPTED")]
    #[lift(FieldSource::Conflict, field = attempted)]
    Attempted(u32),
}
#[test]
fn field_lift_projects_one_field() {
    let payload = ConflictPayload {
        attempted: 7,
        note: "dup".into(),
    };
    let projected: Projected = FieldSource::Conflict(payload).into();
    assert!(matches!(projected, Projected::Attempted(7)));
    assert_eq!(Into::<&'static str>::into(projected.code()), "ATTEMPTED");
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(FieldSource)]
pub enum ProjectedNamed {
    #[rejection(code = "CAPTURED")]
    #[lift(FieldSource::Conflict, field = attempted)]
    Captured { value: u32 },
}
#[test]
fn field_lift_supports_a_named_destination_field() {
    let payload = ConflictPayload {
        attempted: 11,
        note: "dup".into(),
    };
    let projected: ProjectedNamed = FieldSource::Conflict(payload).into();
    assert!(matches!(projected, ProjectedNamed::Captured { value: 11 }));
    assert_eq!(Into::<&'static str>::into(projected.code()), "CAPTURED");
}
