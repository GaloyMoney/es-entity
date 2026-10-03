//! A caller-owned retry loop hands back the narrowed profile, so a caller
//! that retries stops offering `Transient` to its own callers without
//! writing an adapter. The README covers the same ground.
//!
//! The `result_*` tests below exercise the same three narrowings, plus
//! `rejected`/`widen`/`classify`/`record`, through `ResultExt` directly on a
//! `Result` rather than on the bare error value.
use errlanes::{Denied, Fail, Fatal, Fault, ResultExt, Transient, TransientKind, lanes};

type Tf = lanes!(Transient, Fatal);
type Df = lanes!(Denied, Fatal);

#[derive(Debug, Clone, PartialEq, Eq, errlanes::Rejection)]
#[rejection(code = "SMALL")]
struct Small;

fn execute(
    mut inner: impl FnMut() -> Result<u64, Fault<lanes!(Transient, Fatal)>>,
) -> Result<u64, Fault<lanes!(Fatal)>> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match inner() {
            Ok(value) => return Ok(value),
            Err(failure) if failure.is_transient() && attempts < 3 => continue,
            Err(failure) => return Err(failure.narrow_transient(attempts)),
        }
    }
}

#[test]
fn retry_hands_back_the_narrowed_profile() {
    let mut attempts = 0;
    let value = execute(|| {
        attempts += 1;
        if attempts == 1 {
            Err(Transient::new(TransientKind::Deadlock).into())
        } else {
            Ok(42)
        }
    })
    .unwrap();
    assert_eq!(value, 42);
    assert_eq!(attempts, 2);

    // A fatal outcome stays fatal, and the caller can destructure it with a
    // single irrefutable `let` because Fatal is the only lane left.
    let err = execute(|| Err(Fatal::invariant("broken state").into())).unwrap_err();
    let Fault::Fatal(fatal) = err;
    assert_eq!(fatal.context.as_deref(), Some("broken state"));

    // Exhaustion becomes Fatal(Exhausted), with the last transient as source.
    let err = execute(|| Err(Transient::new(TransientKind::Deadlock).into())).unwrap_err();
    let Fault::Fatal(fatal) = err;
    assert_eq!(fatal.kind, errlanes::FatalKind::Exhausted);
    assert!(
        std::error::Error::source(&fatal)
            .unwrap()
            .is::<errlanes::Exhausted>()
    );
}

/// Same regression as `fail::tests::narrow_rejected_does_not_leak_the_rejections_display`,
/// exercised through `ResultExt::narrow_rejected` on a `Result` rather than
/// the bare value.
#[test]
fn result_narrow_rejected_turns_fail_into_fault() {
    let result: Result<u8, Fail<Small, Tf>> = Err(Fail::Rejected(Small));
    let narrowed: Result<u8, Fault<Tf>> = result.narrow_rejected();
    match narrowed {
        Err(Fault::Fatal(fatal)) => {
            assert_eq!(fatal.kind, errlanes::FatalKind::Invariant);
            assert!(
                std::error::Error::source(&fatal)
                    .unwrap()
                    .downcast_ref::<Small>()
                    .is_some(),
                "the rejection must still be reachable for a handler or test to downcast"
            );
            let message = fatal.to_string();
            assert!(
                !message.contains("SMALL"),
                "narrow_rejected's rejection Display must never reach an operator-facing \
                 message; got {message:?}"
            );
        }
        other => panic!("expected Err(Fault::Fatal(Invariant)), got {other:?}"),
    }
}

/// `rejected` is the dual of `narrow_rejected`: the rejection crosses to the
/// `Ok` side as the domain outcome's `Err`, the success value is kept, and
/// only the faults stay on the `Err` side for `?` to carry.
#[test]
fn result_rejected_splits_the_rejection_from_the_faults() {
    let ok: Result<u8, Fail<Small, Tf>> = Ok(7);
    assert_eq!(ok.rejected().unwrap().unwrap(), 7);

    let rejected: Result<u8, Fail<Small, Tf>> = Err(Fail::Rejected(Small));
    assert_eq!(rejected.rejected().unwrap().unwrap_err(), Small);

    let transient: Result<u8, Fail<Small, Tf>> =
        Err(Transient::new(TransientKind::Deadlock).into());
    match transient.rejected() {
        Err(Fault::Transient(t)) => assert_eq!(t.kind, TransientKind::Deadlock),
        other => panic!("expected Err(Fault::Transient), got {other:?}"),
    }
}

/// The call-site shape `rejected` exists for: a wait-forever loop over a
/// timeout-rejecting await. `?` lifts the `Fault` back into the enclosing
/// `Fail` through the blanket `From`, so the loop only has to name the two
/// domain outcomes.
#[test]
fn result_rejected_propagates_faults_through_question_mark() {
    #[derive(Debug, PartialEq, Eq, errlanes::Rejection)]
    #[rejection(code = "TIMED_OUT")]
    struct TimedOut;

    fn wait_forever(
        mut await_once: impl FnMut() -> Result<u64, Fail<TimedOut, Tf>>,
    ) -> Result<u64, Fail<TimedOut, Tf>> {
        loop {
            match await_once().rejected()? {
                Ok(outcome) => break Ok(outcome),
                Err(TimedOut) => continue,
            }
        }
    }

    let mut calls = 0;
    let outcome = wait_forever(|| {
        calls += 1;
        if calls < 3 {
            Err(Fail::Rejected(TimedOut))
        } else {
            Ok(42)
        }
    });
    assert_eq!(outcome.unwrap(), 42);
    assert_eq!(calls, 3);

    let fatal = wait_forever(|| Err(Fatal::invariant("router not started").into())).unwrap_err();
    match fatal {
        Fail::Fatal(f) => assert_eq!(f.context.as_deref(), Some("router not started")),
        other => panic!("expected Fail::Fatal, got {other:?}"),
    }
}

#[test]
fn result_narrow_transient_on_both_carriers() {
    let fault: Result<u8, Fault<Tf>> = Err(Transient::new(TransientKind::Deadlock).into());
    match fault.narrow_transient(3) {
        Err(Fault::Fatal(f)) => assert_eq!(f.kind, errlanes::FatalKind::Exhausted),
        other => panic!("expected Err(Fault::Fatal(Exhausted)), got {other:?}"),
    }

    let fail: Result<u8, Fail<Small, Tf>> = Err(Transient::new(TransientKind::Deadlock).into());
    match fail.narrow_transient(3) {
        Err(Fail::Fatal(f)) => assert_eq!(f.kind, errlanes::FatalKind::Exhausted),
        other => panic!("expected Err(Fail::Fatal(Exhausted)), got {other:?}"),
    }
}

#[test]
fn result_narrow_denied_on_both_carriers() {
    let fault: Result<u8, Fault<Df>> = Err(Denied::default().into());
    match fault.narrow_denied() {
        Err(Fault::Fatal(f)) => assert_eq!(f.kind, errlanes::FatalKind::Denied),
        other => panic!("expected Err(Fault::Fatal(Denied)), got {other:?}"),
    }

    let fail: Result<u8, Fail<Small, Df>> = Err(Denied::default().into());
    match fail.narrow_denied() {
        Err(Fail::Fatal(f)) => assert_eq!(f.kind, errlanes::FatalKind::Denied),
        other => panic!("expected Err(Fail::Fatal(Denied)), got {other:?}"),
    }
}

/// Regression guard for the generic-method form: `.widen()?` must infer its
/// destination from the return type alone, with no turbofish and no let
/// binding's type annotation to lean on.
#[test]
fn result_widen_infers_destination_through_question_mark() {
    fn inner() -> Result<u8, Fail<Small, lanes!(Fatal)>> {
        Err(Fail::Rejected(Small))
    }
    fn outer() -> Result<u8, Fail<Small, Tf>> {
        let value = inner().widen()?;
        Ok(value)
    }
    match outer() {
        Err(Fail::Rejected(Small)) => {}
        other => panic!("expected Err(Fail::Rejected(Small)), got {other:?}"),
    }
}

#[test]
fn result_widen_accepts_turbofish() {
    let r: Result<u8, Fail<Small, lanes!(Fatal)>> = Err(Fail::Rejected(Small));
    let widened = r.widen::<Fail<Small, Tf>>();
    match widened {
        Err(Fail::Rejected(Small)) => {}
        other => panic!("expected Err(Fail::Rejected(Small)), got {other:?}"),
    }
}

#[cfg(feature = "tracing")]
#[test]
fn result_classify_and_record_behave_as_before() {
    // `classify`: ported from `classify.rs`'s
    // `fault_wrapper_enters_fault_by_question_mark`.
    #[derive(Debug, errlanes::Classify)]
    #[classify(fatal(CorruptState), from)]
    struct Stored(std::io::Error);

    fn decode() -> Result<u8, std::io::Error> {
        Err(std::io::Error::other("x"))
    }
    fn fault_fn() -> Result<u8, Fault<Tf>> {
        let _ = decode().classify::<Stored>()?;
        Ok(0)
    }
    match fault_fn() {
        Err(Fault::Fatal(f)) => assert_eq!(f.kind, errlanes::FatalKind::CorruptState),
        other => panic!("expected Err(Fault::Fatal(CorruptState)), got {other:?}"),
    }

    // `record`: ported from `record.rs`'s
    // `record_result_records_onto_the_current_span_and_returns_self` — it
    // records as a side effect and hands the result straight back.
    let span = tracing::info_span!(
        "boundary",
        error = tracing::field::Empty,
        error.lane = tracing::field::Empty,
        error.code = tracing::field::Empty,
        error.level = tracing::field::Empty,
        exception.message = tracing::field::Empty,
        exception.type = tracing::field::Empty,
    );
    let _enter = span.enter();
    let result: Result<(), Fail<Small, Tf>> = Err(Fail::Rejected(Small)).record();
    assert!(matches!(result, Err(Fail::Rejected(Small))));
    let ok: Result<u8, Fail<Small, Tf>> = Ok(7).record();
    assert_eq!(ok.unwrap(), 7);
}
