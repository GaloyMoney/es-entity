//! Smoke tests for `#[derive(errlanes::Classify)]`, covering the shapes from
//! the design handoff's Appendix A: a fault-only wrapper, a mixed wrapper
//! with a `delegate` to a pure rejection and a static lane, `narrow(Denied)`
//! over a delegate, and composition across layers via `.widen()`.

use std::convert::Infallible;

use errlanes::{Classify, Fail, Fault, Rejection, ResultExt, lanes};

type Tf = lanes!(Transient, Fatal);

#[derive(Debug, errlanes::Rejection)]
enum Validation {
    #[rejection(code = "INVALID_AMOUNT")]
    InvalidAmount,
}

#[derive(Debug, errlanes::Classify)]
#[classify(fatal(CorruptState), from)]
struct Stored(#[source] std::io::Error);

// `#[derive(Rejection)]` needs a `code = ".."` on a struct.
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "CONSTRAINT")]
struct ConstraintViolation(&'static str);

#[derive(Debug, errlanes::Classify)]
enum DbWrite {
    #[classify(delegate)]
    Constraint(ConstraintViolation),
    #[classify(transient(OptimisticConflict))]
    Conflict(std::io::Error),
    #[classify(delegate)]
    Other(Stored),
}

impl From<std::io::Error> for DbWrite {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                DbWrite::Constraint(ConstraintViolation("users_email_key"))
            }
            std::io::ErrorKind::Interrupted => DbWrite::Conflict(e),
            _ => DbWrite::Other(Stored(e)),
        }
    }
}

#[derive(Debug, errlanes::Rejection)]
enum JobRejection {
    Dup(ConstraintViolation),
}

impl From<ConstraintViolation> for JobRejection {
    fn from(c: ConstraintViolation) -> Self {
        JobRejection::Dup(c)
    }
}

fn decode() -> Result<u8, std::io::Error> {
    Err(std::io::Error::other("x"))
}

fn insert() -> Result<u8, std::io::Error> {
    Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
}

#[test]
fn fault_wrapper_enters_fault_by_question_mark() {
    fn fault_fn() -> Result<u8, Fault<Tf>> {
        let _ = decode().classify::<Stored>()?;
        Ok(0)
    }
    match fault_fn() {
        Err(Fault::Fatal(f)) => assert_eq!(f.kind, errlanes::FatalKind::CorruptState),
        other => panic!("expected Fatal(CorruptState), got {other:?}"),
    }
}

#[test]
fn mixed_wrapper_enters_fail_by_question_mark_total_absorption() {
    fn fail_fn() -> Result<u8, Fail<JobRejection, Tf>> {
        let _ = insert().classify::<DbWrite>()?;
        Ok(0)
    }
    match fail_fn() {
        Err(Fail::Rejected(JobRejection::Dup(c))) => assert_eq!(c.0, "users_email_key"),
        other => panic!("expected Rejected(Dup), got {other:?}"),
    }
}

#[test]
fn static_lane_variant_is_transient() {
    let e = std::io::Error::from(std::io::ErrorKind::Interrupted);
    let wrapped: DbWrite = e.into();
    match wrapped.classify() {
        Fail::Transient(t) => assert_eq!(t.kind, errlanes::TransientKind::OptimisticConflict),
        other => panic!("expected Transient(OptimisticConflict), got {other:?}"),
    }
}

#[test]
fn delegate_through_a_fault_only_payload_widens_its_lanes() {
    let e = std::io::Error::other("disk full");
    let wrapped: DbWrite = e.into();
    match wrapped.classify() {
        Fail::Fatal(f) => assert_eq!(f.kind, errlanes::FatalKind::CorruptState),
        other => panic!("expected Fatal(CorruptState), got {other:?}"),
    }
}

#[test]
fn a_bare_rejection_is_classify_via_the_blanket() {
    let r = Validation::InvalidAmount;
    let fail: Fail<Validation> = r.into();
    assert!(matches!(fail, Fail::Rejected(Validation::InvalidAmount)));
}

#[test]
fn narrow_rejected_turns_a_fail_into_a_fault() {
    let wrapped: DbWrite = insert().unwrap_err().into();
    let fail: Fail<ConstraintViolation, Tf> = Classify::classify(wrapped);
    let fault = fail.map_err_narrow();
    assert!(fault.is_fatal());
}

trait MapErrNarrow<L: errlanes::LaneProfile> {
    fn map_err_narrow(self) -> Fault<L>;
}
impl<D: Rejection, L: errlanes::LaneProfile<Fatal = errlanes::Fatal>> MapErrNarrow<L>
    for Fail<D, L>
{
    fn map_err_narrow(self) -> Fault<L> {
        self.narrow_rejected()
    }
}

#[test]
fn infallible_rejected_slot_is_a_rejected_slot() {
    fn assert_rejected_slot<T: errlanes::RejectedSlot>() {}
    assert_rejected_slot::<Infallible>();
    assert_rejected_slot::<Validation>();
}

// Pure mode: no lane/delegate anywhere, so `derive(Classify)` must emit
// exactly what `derive(Rejection)` emits (byte-identical grammar, same
// engine) and `Classify` comes from the blanket, not a direct impl.
#[derive(Debug, errlanes::Classify)]
enum PureViaClassify {
    #[classify(code = "TOO_SMALL")]
    TooSmall,
}

#[test]
fn pure_mode_delegates_to_the_rejection_engine() {
    let r = PureViaClassify::TooSmall;
    assert_eq!(r.code().to_string(), "TOO_SMALL");
    // Classify comes from the blanket `impl<R: Rejection> Classify for R`,
    // not a direct impl — a direct impl would conflict with it (E0119).
    let fail: Fail<PureViaClassify> = PureViaClassify::TooSmall.into();
    assert!(matches!(fail, Fail::Rejected(PureViaClassify::TooSmall)));
}

// A mixed wrapper with both a `Denied` and a `Fatal` lane (so `narrow_denied`
// has somewhere to land), narrowed away over a `delegate`.
#[derive(Debug, errlanes::Classify)]
enum Unauthorized {
    #[classify(denied)]
    NotOurs,
    #[classify(fatal(Config))]
    Misconfigured,
}

#[derive(Debug, errlanes::Classify)]
enum Proxy {
    #[classify(delegate, narrow(Denied))]
    Upstream(Unauthorized),
}

#[test]
fn narrow_denied_turns_a_delegated_denial_into_fatal() {
    let wrapped = Proxy::Upstream(Unauthorized::NotOurs);
    match wrapped.classify() {
        Fail::Fatal(f) => assert_eq!(f.kind, errlanes::FatalKind::Denied),
        other => panic!("expected Fatal(Denied), got {other:?}"),
    }
}

#[test]
fn a_fatal_variant_alongside_a_denied_one_stays_fatal() {
    let wrapped = Proxy::Upstream(Unauthorized::Misconfigured);
    match wrapped.classify() {
        Fail::Fatal(f) => assert_eq!(f.kind, errlanes::FatalKind::Config),
        other => panic!("expected Fatal(Config), got {other:?}"),
    }
}

// Composition across layers: each layer declares one lift against the type
// it actually receives.
#[derive(Debug, errlanes::Rejection)]
enum Api {
    #[rejection(from)]
    Job(JobRejection),
}

#[test]
fn composition_across_layers_widens_through_each_hop() {
    fn repo_insert() -> Result<u8, Fail<ConstraintViolation, Tf>> {
        let _ = insert().classify::<DbWrite>()?;
        Ok(0)
    }
    fn service() -> Result<u8, Fail<JobRejection, Tf>> {
        let _ = repo_insert().widen()?;
        Ok(0)
    }
    fn api() -> Result<u8, Fail<Api, Tf>> {
        let _ = service().widen()?;
        Ok(0)
    }
    match api() {
        Err(Fail::Rejected(Api::Job(JobRejection::Dup(c)))) => assert_eq!(c.0, "users_email_key"),
        other => panic!("expected Rejected(Job(Dup)), got {other:?}"),
    }
}

// Regression: `Rejected` is folded via `RejectedUnion` across every
// `delegate`, not read off whichever variant happens to be declared first.
// `Other` (never rejects) is declared *before* `Constraint` (the one
// genuine rejection) here — the reverse of `DbWrite` above.
#[derive(Debug, errlanes::Classify)]
enum DbWriteReordered {
    #[classify(delegate)]
    Other(Stored),
    #[classify(delegate)]
    Constraint(ConstraintViolation),
}

#[test]
fn rejected_is_folded_order_independently_across_delegates() {
    fn assert_rejected_is<R: errlanes::RejectedSlot>() {}
    assert_rejected_is::<<DbWriteReordered as Classify>::Rejected>();
    let _: <DbWriteReordered as Classify>::Rejected = ConstraintViolation("users_email_key");

    let wrapped = DbWriteReordered::Constraint(ConstraintViolation("users_email_key"));
    match wrapped.classify() {
        Fail::Rejected(ConstraintViolation(v)) => assert_eq!(v, "users_email_key"),
        other => panic!("expected Rejected, got {other:?}"),
    }

    let wrapped = DbWriteReordered::Other(Stored(std::io::Error::other("x")));
    match wrapped.classify() {
        Fail::Fatal(f) => assert_eq!(f.kind, errlanes::FatalKind::CorruptState),
        other => panic!("expected Fatal(CorruptState), got {other:?}"),
    }
}

// Named fields: a `#[source]` field among siblings (the shape errlanes also
// treats as the cause) and a lone named field both `delegate`; `from` on the
// lone one builds `Self::Wrapped { inner: value }`.
#[derive(Debug, errlanes::Classify)]
enum NamedPayloads {
    #[classify(delegate)]
    Decode {
        sequence: i32,
        #[source]
        source: Stored,
    },
    #[classify(delegate, from)]
    Wrapped { inner: ConstraintViolation },
}

#[derive(Debug, errlanes::Classify)]
#[classify(fatal(Config), from)]
struct NamedStruct {
    #[source]
    source: std::io::Error,
}

#[test]
fn a_source_field_among_siblings_delegates() {
    let wrapped = NamedPayloads::Decode {
        sequence: 7,
        source: Stored(std::io::Error::other("x")),
    };
    match wrapped.classify() {
        Fail::Fatal(f) => assert_eq!(f.kind, errlanes::FatalKind::CorruptState),
        other => panic!("expected Fatal(CorruptState), got {other:?}"),
    }
}

#[test]
fn a_lone_named_field_delegates_and_converts_with_from() {
    let wrapped: NamedPayloads = ConstraintViolation("users_email_key").into();
    match wrapped.classify() {
        Fail::Rejected(ConstraintViolation(v)) => assert_eq!(v, "users_email_key"),
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[test]
fn a_named_struct_field_converts_with_from() {
    let wrapped: NamedStruct = std::io::Error::other("x").into();
    match wrapped.classify() {
        Fail::Fatal(f) => assert_eq!(f.kind, errlanes::FatalKind::Config),
        other => panic!("expected Fatal(Config), got {other:?}"),
    }
}
