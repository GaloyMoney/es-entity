//! `Fail<Infallible, L>::into_fault`: the carrier switch for a `Fail` that
//! cannot reject, lane for lane and without a turbofish.

use std::{convert::Infallible, error::Error};

use errlanes::{
    Classify, Denied, Fail, Fatal, FatalKind, Fault, Lane, Transient, TransientKind, lanes,
};

type All = lanes!(Denied, Transient, Fatal);

#[derive(Debug, errlanes::Classify)]
enum Source {
    #[classify(denied)]
    Forbidden,
    #[classify(transient(Congestion))]
    Busy,
    #[classify(fatal(Config))]
    Misconfigured,
}

#[test]
fn fail_infallible_into_fault_is_lane_for_lane() {
    let fatal: Fail<Infallible, All> =
        Fail::Fatal(Fatal::new(FatalKind::Config).with_context("fctx"));
    let fault: Fault<All> = fatal.into_fault();
    assert_eq!(fault.lane(), Lane::Fatal);
    match fault {
        Fault::Fatal(f) => {
            assert_eq!(f.kind, FatalKind::Config);
            assert_eq!(f.context.as_deref(), Some("fctx"));
        }
        other => panic!("expected Fatal(Config), got {other:?}"),
    }

    let transient: Fail<Infallible, All> =
        Fail::Transient(Transient::new(TransientKind::Congestion).with_context("tctx"));
    match transient.into_fault() {
        Fault::Transient(t) => {
            assert_eq!(t.kind, TransientKind::Congestion);
            assert_eq!(t.context.as_deref(), Some("tctx"));
        }
        other => panic!("expected Transient, got {other:?}"),
    }

    let denied: Fail<Infallible, All> = Fail::Denied(Denied::new());
    assert_eq!(denied.into_fault().lane(), Lane::Denied);
}

#[test]
fn a_source_survives_the_switch() {
    let io = std::io::Error::other("bad bytes");
    let fail: Fail<Infallible, All> = Fail::Fatal(Fatal::from_error(FatalKind::Config, io));
    match fail.into_fault() {
        Fault::Fatal(f) => assert_eq!(f.source().expect("source kept").to_string(), "bad bytes"),
        other => panic!("expected Fatal, got {other:?}"),
    }
}

#[test]
fn a_classified_source_narrows_and_switches_without_a_turbofish() {
    fn hook(e: Source) -> Fault<lanes!(Transient, Fatal)> {
        e.classify().narrow_denied().into_fault()
    }
    match hook(Source::Forbidden) {
        Fault::Fatal(f) => assert_eq!(f.kind, FatalKind::Denied),
        other => panic!("expected Fatal(Denied), got {other:?}"),
    }
    assert!(hook(Source::Busy).is_transient());
    match hook(Source::Misconfigured) {
        Fault::Fatal(f) => assert_eq!(f.kind, FatalKind::Config),
        other => panic!("expected Fatal(Config), got {other:?}"),
    }
}
