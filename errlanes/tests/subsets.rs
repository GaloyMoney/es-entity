use errlanes::{
    Fail, Fatal, FatalKind, Fault, Lane, Rejection, Transient, TransientKind, WidenResult, lanes,
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
        .with_retry_after(std::time::Duration::from_millis(7))
        .with_source(std::io::Error::other("deadlock"));
    let original_source = transient.source_arc().unwrap().clone();
    let result: Result<(), Fault<lanes!(Transient)>> = Err(transient.into());
    let widened: Result<(), Fault<lanes!(Transient, Fatal, Denied)>> = result.widen();
    let Fault::Transient(error) = widened.unwrap_err() else {
        panic!("expected transient");
    };
    assert_eq!(error.kind, TransientKind::Deadlock);
    assert_eq!(error.context.as_deref(), Some("retry transaction"));
    assert_eq!(error.retry_after, Some(std::time::Duration::from_millis(7)));
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

#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
pub enum Child {
    #[error("taken {0}")]
    Taken(String),
    #[error("range {low}..{high}")]
    Range { low: u32, high: u32 },
    #[error("unit")]
    Unit,
}
#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Child)]
pub enum Parent {
    #[error("taken {0}")]
    #[lift(Child::Taken)]
    Taken(String),
    #[error("range {low}..{high}")]
    #[lift(Child::Range)]
    Range { low: u32, high: u32 },
    #[error("unit")]
    #[lift(Child::Unit)]
    Unit,
}
#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, unhandled = fatal)]
pub enum Partial {
    #[error("taken {0}")]
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
    // Settling a profile with no transient lane is the identity on the value,
    // and its type is already a `Fail` -- there is nothing to convert back.
    let restored: Fail<Parent, lanes!()> = lifted.settle(1);
    assert!(matches!(restored, Fail::Rejected(Parent::Unit)));
}
#[test]
fn borrowed_settlement_needs_no_uninhabited_arms() {
    // `lane()` and the borrowed accessors cover every borrowed inspection, so
    // no caller writes `match *never {}`.
    fn inspect(value: &Fail<Child, lanes!(Fatal)>) -> Lane {
        value.lane()
    }
    let value: Fail<Child, lanes!(Fatal)> = Fatal::invariant("broken").into();
    let settled = value.settle(1);
    assert_eq!(inspect(&settled), Lane::Fatal);
    assert!(settled.as_fatal().is_some());
    assert!(settled.as_rejected().is_none());
}
#[test]
fn widening_preserves_transient_details_and_boxed_marker() {
    let original: Fail<Child, lanes!(Transient)> = Transient::new(TransientKind::Deadlock)
        .with_source(std::io::Error::other("source"))
        .with_context("operation")
        .into();
    let widened: Fail<Parent, lanes!(Fatal, Transient)> = original.widen();
    assert_eq!(errlanes::lane_of(&widened), Some(Lane::Transient));
    let settled: Fail<Parent, lanes!(Fatal)> = widened.settle(3);
    let fatal = settled.as_fatal().expect("expected exhaustion");
    assert_eq!(fatal.kind, errlanes::FatalKind::Exhausted);
    let e = fatal
        .source()
        .and_then(|s| s.downcast_ref::<errlanes::Exhausted>())
        .expect("exhaustion is the fatal's source");
    assert_eq!(e.attempts, 3);
    assert_eq!(e.last.kind, TransientKind::Deadlock);
    assert!(e.last.source().unwrap().is::<std::io::Error>());
    let boxed: Box<dyn Error + Send + Sync> = Box::new(settled);
    assert_eq!(errlanes::lane_of(boxed.as_ref()), Some(Lane::Fatal));
}
#[test]
fn infallible_and_boxed_conversions_are_coherent() {
    #[allow(unreachable_code)]
    fn convert(x: Infallible) -> Fault<lanes!()> {
        x.into()
    }
    let _ = convert;
    let boxed: Box<dyn Error + Send + Sync> = Box::new(std::io::Error::other("source"));
    let fault: Fault<lanes!(Fatal)> = boxed.into();
    assert_eq!(fault.lane(), Lane::Fatal);
    assert_eq!(Into::<&'static str>::into(Child::Unit.code()), "UNIT");
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn retry_respects_subset_and_preserves_retry_after() {
    use std::{cell::Cell, time::Duration};
    let attempts = Cell::new(0);
    let sleeps = Cell::new(0);
    let policy = errlanes::RetryPolicy {
        max_attempts: 3,
        ..Default::default()
    };
    let result = errlanes::retry_with(
        &policy,
        || {
            attempts.set(attempts.get() + 1);
            async {
                // `lanes!(Transient)` alone is no longer retryable: settling it
                // would have nowhere to put the exhaustion. Anything worth
                // retrying can fail permanently, so it must admit Fatal.
                Err::<(), Fail<Child, lanes!(Transient, Fatal)>>(
                    Transient::new(TransientKind::Deadlock)
                        .with_retry_after(Duration::from_millis(7))
                        .into(),
                )
            }
        },
        |delay| {
            assert_eq!(delay, Duration::from_millis(7));
            sleeps.set(sleeps.get() + 1);
            async {}
        },
    )
    .await;
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
    assert_eq!(attempts.get(), 3);
    assert_eq!(sleeps.get(), 2);
    let result = errlanes::retry_with(
        &policy,
        || async {
            Err::<(), Fail<Child, lanes!(Fatal)>>(Fatal::invariant("stored corruption").into())
        },
        |_| async { panic!("a fatal-only profile cannot retry") },
    )
    .await;
    assert!(matches!(result, Err(Fail::Fatal(_))));
}

#[test]
fn transparent_subset_wrapper_retains_lane_markers() {
    #[derive(Debug, thiserror::Error)]
    #[error(transparent)]
    struct Wrapper(Fail<Child, lanes!(Denied, Fatal)>);
    let boxed: Box<dyn Error + Send + Sync> = Box::new(Wrapper(errlanes::Denied::default().into()));
    assert_eq!(errlanes::lane_of(boxed.as_ref()), Some(Lane::Denied));
    let boxed: Box<dyn Error + Send + Sync> = Box::new(Wrapper(Fatal::invariant("broken").into()));
    assert_eq!(errlanes::lane_of(boxed.as_ref()), Some(Lane::Fatal));
}

#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Child)]
pub enum Renamed {
    #[lift(Child::Taken)]
    #[error("renamed {0}")]
    RenamedTaken(String),
    #[lift(Child::Range)]
    #[error("range {low}..{high}")]
    RenamedRange { low: u32, high: u32 },
    #[lift(Child::Unit)]
    #[error("unit")]
    RenamedUnit,
}
#[test]
fn explicit_strict_rename_preserves_leaf_code() {
    let parent: Renamed = Child::Taken("id".into()).into();
    assert_eq!(Into::<&'static str>::into(parent.code()), "TAKEN");
}

#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Child, strict)]
pub enum Transformed {
    #[error("canonical {0}")]
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

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Other {
    #[error("other")]
    #[rejection(code = "OTHER", level = "warn")]
    Unit,
}
#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Other)]
#[lift(Child)]
pub enum Combined {
    #[error("unit")]
    #[rejection(code = "SHARED")]
    #[lift(Other::Unit)]
    #[lift(Child::Unit)]
    Unit,
    #[error("taken {0}")]
    #[lift(Child::Taken)]
    Taken(String),
    #[error("range {low}..{high}")]
    #[lift(Child::Range)]
    Range { low: u32, high: u32 },
}
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Delegated {
    #[error(transparent)]
    #[rejection(delegate)]
    Other(#[from] Other),
}
#[test]
fn multiple_sources_and_explicit_delegation() {
    assert!(matches!(Combined::from(Other::Unit), Combined::Unit));
    assert!(matches!(Combined::from(Child::Unit), Combined::Unit));
    let delegated = Delegated::from(Other::Unit);
    assert_eq!(delegated.level(), errlanes::Level::Warn);
    assert_eq!(Into::<&'static str>::into(delegated.code()), "OTHER");
}
