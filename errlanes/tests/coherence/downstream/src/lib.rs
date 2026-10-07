//! The "consumer" side of the two-crate coherence fixture. It owns carriers of
//! its own and absorbs the upstream ones, so every claim here is a claim about
//! what a *downstream* crate may write (orphan rules, overlap against errlanes'
//! blankets), which a single-crate test cannot pin.

use std::error::Error;

use errlanes::{
    Carrier, Denied, Exhausted, Fail, Fatal, Fault, Laned, ResultExt, Transient, TransientKind,
    lanes,
};
use upstream::{DepositError, OnlyFatal, WriteError};

#[derive(Debug, Clone, errlanes::Rejection)]
pub enum CustomerRejection {
    CustomerIsClosed,
    #[rejection(from)]
    Deposit(upstream::DepositRejection),
}

/// Strict-lift is derived by `#[rejection(from)]` above; this one is partial:
/// it maps only `AccountFrozen`, and demotes everything else to `Fatal`.
#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(upstream::DepositRejection, unhandled = fatal)]
pub enum FrozenOnly {
    #[lift(upstream::DepositRejection::AccountFrozen)]
    Frozen,
}

#[derive(Debug, errlanes::Classify)]
#[classify(fatal(CorruptState))]
#[error("could not decode stored state")]
pub struct Stored(#[source] std::io::Error);

fn stored() -> Stored {
    Stored(std::io::Error::other("bad bytes"))
}

/// The coherence fixture's `from(..)` carriers are declared in this crate, so
/// the blanket inbound `From` is provably disjoint from each listed one. A
/// carrier declared in *another* crate cannot be listed: see
/// `tests/coherence/from_foreign_carrier.rs`.
#[errlanes::fault(Transient, Fatal)]
pub struct HostFault;

#[errlanes::fault(Transient, Fatal)]
pub struct RepoFault;

fn host_op() -> Result<u8, HostFault> {
    Ok(7)
}

fn host_transient() -> Result<u8, HostFault> {
    Err(HostFault::Transient(errlanes::Transient::new(
        errlanes::TransientKind::Deadlock,
    )))
}

fn repo_op() -> Result<u8, RepoFault> {
    Ok(8)
}

/// Two carriers from another crate behind two `#[from]`s: the old E0119.
#[derive(Debug, thiserror::Error)]
pub enum HandRolledUpstream {
    #[error(transparent)]
    Host(#[from] upstream::HostFault),
    #[error(transparent)]
    Repo(#[from] upstream::RepoFault),
}

/// A carrier with a lane the upstream ones lack, absorbing both of them.
#[errlanes::fault(Denied, Transient, Fatal; from(HostFault, RepoFault))]
pub struct PartyFault;

/// A fail-like carrier with a declared rejection.
#[errlanes::fail(CustomerRejection; Transient, Fatal; from(HostFault))]
pub struct CustomerError;

/// `upstream::WriteError<R>` with `from(HostFault)` added: a downstream copy.
#[errlanes::fail(R; Transient, Fatal; from(HostFault))]
pub struct LocalWriteError<R>;

/// A carrier declared by derive on a hand-written enum, with a doc comment on
/// a variant (the reason that form exists).
#[derive(Debug, errlanes::Carrier)]
#[carrier(from(HostFault))]
pub enum Handwritten {
    /// the subject may not do this
    Denied(errlanes::Denied),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

/// A hand-rolled `thiserror` enum that absorbs two carriers: the old E0119.
#[derive(Debug, thiserror::Error)]
pub enum HandRolled {
    #[error(transparent)]
    Host(#[from] HostFault),
    #[error(transparent)]
    Repo(#[from] RepoFault),
}

// ---------------------------------------------------------------------------
// Effect bound: `E: Error + From<HostFault> + Send + Sync + 'static`
// ---------------------------------------------------------------------------

fn effect_error<E: Error + From<HostFault> + Send + Sync + 'static>() {}

fn in_effect<E: Error + From<HostFault>>() -> Result<u8, E> {
    let v = host_op()?;
    Ok(v)
}

fn effect_bound_holds_everywhere() {
    effect_error::<HostFault>();
    effect_error::<Fault<lanes!(Transient, Fatal)>>();
    effect_error::<Fault<lanes!(Denied, Transient, Fatal)>>();
    effect_error::<Fail<CustomerRejection, lanes!(Transient, Fatal)>>();
    effect_error::<Fail<CustomerRejection, lanes!(Denied, Transient, Fatal)>>();
    effect_error::<PartyFault>();
    effect_error::<CustomerError>();
    effect_error::<LocalWriteError<CustomerRejection>>();
    effect_error::<Handwritten>();
    effect_error::<HandRolled>();

    assert_eq!(in_effect::<HostFault>().unwrap(), 7);
    assert_eq!(in_effect::<Fault<lanes!(Transient, Fatal)>>().unwrap(), 7);
    assert_eq!(
        in_effect::<Fault<lanes!(Denied, Transient, Fatal)>>().unwrap(),
        7
    );
    assert_eq!(
        in_effect::<Fail<CustomerRejection, lanes!(Transient, Fatal)>>().unwrap(),
        7
    );
    assert_eq!(
        in_effect::<Fail<CustomerRejection, lanes!(Denied, Transient, Fatal)>>().unwrap(),
        7
    );
    assert_eq!(in_effect::<PartyFault>().unwrap(), 7);
    assert_eq!(in_effect::<CustomerError>().unwrap(), 7);
    assert_eq!(
        in_effect::<LocalWriteError<CustomerRejection>>().unwrap(),
        7
    );
    assert_eq!(in_effect::<Handwritten>().unwrap(), 7);
    assert_eq!(in_effect::<HandRolled>().unwrap(), 7);
}

// ---------------------------------------------------------------------------
// Inbound `?`
// ---------------------------------------------------------------------------

fn deadlock() -> Transient {
    Transient::new(TransientKind::Deadlock)
}

fn exhausted() -> Exhausted {
    Exhausted {
        attempts: 3,
        last: deadlock(),
    }
}

fn into_host_from_transient() -> Result<(), HostFault> {
    Err(deadlock())?;
    Ok(())
}

fn into_host_from_fatal() -> Result<(), HostFault> {
    Err(Fatal::invariant("boom"))?;
    Ok(())
}

fn into_host_from_wrapper() -> Result<(), HostFault> {
    Err(stored())?;
    Ok(())
}

fn into_host_from_narrower_fault() -> Result<(), HostFault> {
    Err(Fault::<lanes!(Fatal)>::Fatal(Fatal::invariant("narrow")))?;
    Ok(())
}

fn into_host_from_same_profile_fault() -> Result<(), HostFault> {
    Err(Fault::<lanes!(Transient, Fatal)>::Transient(deadlock()))?;
    Ok(())
}

fn into_host_from_exhausted() -> Result<(), HostFault> {
    Err(exhausted())?;
    Ok(())
}

fn into_party_from_denied() -> Result<(), PartyFault> {
    Err(Denied::new())?;
    Ok(())
}

fn into_party_from_host() -> Result<(), PartyFault> {
    host_transient()?;
    Ok(())
}

fn into_party_from_repo() -> Result<(), PartyFault> {
    Err(RepoFault::Fatal(Fatal::invariant("repo")))?;
    Ok(())
}

fn into_customer_from_rejection() -> Result<(), CustomerError> {
    Err(CustomerRejection::CustomerIsClosed)?;
    Ok(())
}

fn into_customer_from_fail_view() -> Result<(), CustomerError> {
    Err(
        Fail::<CustomerRejection, lanes!(Transient, Fatal)>::Rejected(
            CustomerRejection::CustomerIsClosed,
        ),
    )?;
    Ok(())
}

fn into_customer_from_upstream_rejection() -> Result<(), CustomerError> {
    Err(
        Fail::<upstream::DepositRejection, lanes!(Transient, Fatal)>::Rejected(
            upstream::DepositRejection::AccountFrozen,
        )
        .widen::<CustomerRejection, lanes!(Transient, Fatal)>(),
    )?;
    Ok(())
}

fn into_customer_from_upstream_carrier() -> Result<(), CustomerError> {
    Err::<(), _>(upstream::frozen())
        .widen::<Fail<CustomerRejection, lanes!(Transient, Fatal)>>()?;
    Ok(())
}

fn into_customer_from_wrapper() -> Result<(), CustomerError> {
    Err(stored())?;
    Ok(())
}

fn into_customer_from_payload() -> Result<(), CustomerError> {
    Err(deadlock())?;
    Ok(())
}

fn into_customer_from_narrower_fail() -> Result<(), CustomerError> {
    Err(Fail::<CustomerRejection, lanes!(Fatal)>::Fatal(
        Fatal::invariant("narrow"),
    ))?;
    Ok(())
}

fn into_customer_from_host() -> Result<(), CustomerError> {
    host_transient()?;
    Ok(())
}

fn into_write_from_rejection<R: errlanes::Rejection>(r: R) -> Result<(), WriteError<R>> {
    Err(r)?;
    Ok(())
}

fn into_write_from_payload<R: errlanes::Rejection>() -> Result<(), WriteError<R>> {
    Err(deadlock())?;
    Ok(())
}

fn into_write_from_wrapper<R: errlanes::Rejection>() -> Result<(), WriteError<R>> {
    Err(stored())?;
    Ok(())
}

fn into_local_write_from_host() -> Result<(), LocalWriteError<CustomerRejection>> {
    host_transient()?;
    Ok(())
}

fn into_handwritten_from_host() -> Result<(), Handwritten> {
    host_transient()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Outbound `?`
// ---------------------------------------------------------------------------

fn host_into_wide_fault() -> Result<(), Fault<lanes!(Denied, Transient, Fatal)>> {
    host_transient()?;
    Ok(())
}

fn repo_into_all_lanes_fault() -> Result<(), Fault> {
    repo_op()?;
    Ok(())
}

fn host_into_fail() -> Result<(), Fail<CustomerRejection>> {
    host_transient()?;
    Ok(())
}

fn customer_into_fail() -> Result<(), Fail<CustomerRejection>> {
    into_customer_from_host()?;
    Ok(())
}

fn write_into_fail<R: errlanes::Rejection>(w: Result<(), WriteError<R>>) -> Result<(), Fail<R>> {
    w?;
    Ok(())
}

fn single_lane_into_bare_fatal() -> Result<(), Fatal> {
    Err(OnlyFatal::Fatal(Fatal::invariant("only")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Widen
// ---------------------------------------------------------------------------

fn widen_fail_infers() -> Result<(), Fail<CustomerRejection>> {
    let r: Result<(), Fail<CustomerRejection, lanes!(Transient, Fatal)>> =
        Err(Fail::Rejected(CustomerRejection::CustomerIsClosed));
    r.widen()?;
    Ok(())
}

fn widen_fault_infers() -> Result<(), Fault> {
    let r: Result<(), Fault<lanes!(Transient, Fatal)>> = Err(Fault::Transient(deadlock()));
    r.widen()?;
    Ok(())
}

fn widen_tail_position() -> Result<(), Fault> {
    let r: Result<(), Fault<lanes!(Transient, Fatal)>> = Err(Fault::Transient(deadlock()));
    r.widen()
}

fn widen_carrier_source() -> Result<(), Fault> {
    host_transient().widen()?;
    Ok(())
}

fn widen_carrier_into_other_carrier_without_a_list() -> Result<(), PartyFault> {
    // `PartyFault` lists `HostFault`, but a carrier that is not listed goes
    // through the built-in it widens to, then the outer `?` absorbs it.
    OnlyFatalAsResult().widen::<Fault<lanes!(Denied, Transient, Fatal)>>()?;
    Ok(())
}

#[allow(non_snake_case)]
fn OnlyFatalAsResult() -> Result<(), OnlyFatal> {
    Err(OnlyFatal::Fatal(Fatal::invariant("only")))
}

fn partial_lift_from_fail() -> Result<(), Fail<FrozenOnly, lanes!(Transient, Fatal)>> {
    let r: Result<(), Fail<upstream::DepositRejection, lanes!(Transient, Fatal)>> = Err(
        Fail::Rejected(upstream::DepositRejection::DailyLimitExceeded),
    );
    r.widen()?;
    Ok(())
}

fn partial_lift_from_carrier() -> Result<(), Fail<FrozenOnly, lanes!(Transient, Fatal)>> {
    Err::<(), DepositError>(upstream::frozen()).widen()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

fn by_value(e: HostFault) -> &'static str {
    match e {
        HostFault::Transient(_) => "retry",
        HostFault::Fatal(_) => "page",
    }
}

fn generic_match<R: errlanes::Rejection>(e: WriteError<R>) -> Option<R> {
    match e {
        WriteError::Rejected(r) => Some(r),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Generic code
// ---------------------------------------------------------------------------

/// Hand-rolled retry loop generic over anything `Laned`: a carrier or a bare
/// `Fault`-returning read alike, with no retry machinery to depend on.
fn retry<T, E: Laned>(mut op: impl FnMut() -> Result<T, E>) -> Result<T, E::WithoutTransient> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match op() {
            Ok(t) => return Ok(t),
            Err(e) if Laned::is_transient(&e) && attempts < 3 => continue,
            Err(e) => return Err(Laned::narrow_transient(e, attempts)),
        }
    }
}

#[errlanes::instrument]
fn instrumented() -> Result<u8, HostFault> {
    host_transient()
}

fn lane_of_carrier<C: Carrier>(c: &C) -> errlanes::Lane {
    c.lanes().lane()
}

/// Stand-in for a generated `find_by_id`: reads never reject, so the
/// signature is `Fault`, not `Fail<D>`.
async fn fake_find_by_id(fail: bool) -> Result<(), Fault> {
    if fail { Err(deadlock().into()) } else { Ok(()) }
}

async fn find_by_id_into_fail_view() -> Result<(), Fail<CustomerRejection>> {
    fake_find_by_id(true).await?;
    Ok(())
}

async fn find_by_id_into_carrier() -> Result<(), PartyFault> {
    fake_find_by_id(true).await?;
    Ok(())
}

/// Exercises every positive claim of the two-crate coherence fixture.
pub fn smoke() {
    effect_bound_holds_everywhere();

    // Inbound.
    assert!(into_host_from_transient().unwrap_err().is_transient());
    assert!(into_host_from_fatal().unwrap_err().is_fatal());
    assert!(into_host_from_wrapper().unwrap_err().is_fatal());
    assert!(into_host_from_narrower_fault().unwrap_err().is_fatal());
    assert!(
        into_host_from_same_profile_fault()
            .unwrap_err()
            .is_transient()
    );
    assert!(into_host_from_exhausted().unwrap_err().is_fatal());
    assert!(into_party_from_denied().unwrap_err().is_denied());
    assert!(into_party_from_host().unwrap_err().is_transient());
    assert!(into_party_from_repo().unwrap_err().is_fatal());
    assert!(into_customer_from_rejection().unwrap_err().is_rejected());
    assert!(into_customer_from_fail_view().unwrap_err().is_rejected());
    assert!(
        into_customer_from_upstream_rejection()
            .unwrap_err()
            .is_rejected()
    );
    assert!(
        into_customer_from_upstream_carrier()
            .unwrap_err()
            .is_rejected()
    );
    assert!(into_customer_from_wrapper().unwrap_err().is_fatal());
    assert!(into_customer_from_payload().unwrap_err().is_transient());
    assert!(into_customer_from_narrower_fail().unwrap_err().is_fatal());
    assert!(into_customer_from_host().unwrap_err().is_transient());
    assert!(
        into_write_from_rejection(CustomerRejection::CustomerIsClosed)
            .unwrap_err()
            .is_rejected()
    );
    assert!(
        into_write_from_payload::<CustomerRejection>()
            .unwrap_err()
            .is_transient()
    );
    assert!(
        into_write_from_wrapper::<CustomerRejection>()
            .unwrap_err()
            .is_fatal()
    );
    assert!(into_local_write_from_host().unwrap_err().is_transient());
    assert!(into_handwritten_from_host().unwrap_err().is_transient());

    // Outbound.
    assert!(host_into_wide_fault().unwrap_err().is_transient());
    assert!(repo_into_all_lanes_fault().is_ok());
    assert!(host_into_fail().unwrap_err().is_transient());
    assert!(customer_into_fail().unwrap_err().is_transient());
    assert!(
        write_into_fail(Err(WriteError::Rejected(
            CustomerRejection::CustomerIsClosed
        )))
        .unwrap_err()
        .as_rejected()
        .is_some()
    );
    assert!(single_lane_into_bare_fatal().is_err());

    // Widen.
    assert!(widen_fail_infers().unwrap_err().as_rejected().is_some());
    assert!(widen_fault_infers().unwrap_err().is_transient());
    assert!(widen_tail_position().unwrap_err().is_transient());
    assert!(widen_carrier_source().unwrap_err().is_transient());
    assert!(
        widen_carrier_into_other_carrier_without_a_list()
            .unwrap_err()
            .is_fatal()
    );
    assert!(partial_lift_from_fail().unwrap_err().is_fatal());
    assert!(
        partial_lift_from_carrier()
            .unwrap_err()
            .as_rejected()
            .is_some()
    );

    // Matching.
    assert_eq!(by_value(HostFault::Transient(deadlock())), "retry");
    assert_eq!(by_value(HostFault::Fatal(Fatal::invariant("x"))), "page");
    assert!(matches!(
        &HostFault::Transient(deadlock()),
        HostFault::Transient(_)
    ));
    assert!(generic_match(WriteError::Rejected(CustomerRejection::CustomerIsClosed)).is_some());
    assert!(
        generic_match(WriteError::<CustomerRejection>::Fatal(Fatal::invariant(
            "x"
        )))
        .is_none()
    );

    // Generic code.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    assert!(rt.block_on(find_by_id_into_fail_view()).is_err());
    assert!(rt.block_on(find_by_id_into_carrier()).is_err());

    let narrowed = retry(host_transient);
    // A narrowed carrier is its narrowed built-in: no transient arm at all.
    // Exhaustion arrives in the fatal lane, tagged `FatalKind::Exhausted`.
    let lane = match narrowed {
        Err(Fault::Denied(_)) => "denied",
        Err(Fault::Fatal(f)) if f.kind == errlanes::FatalKind::Exhausted => "exhausted",
        Err(Fault::Fatal(_)) => "fatal",
        Ok(_) => "ok",
    };
    assert_eq!(lane, "exhausted");

    let narrowed = retry(|| Err::<(), _>(upstream::transient()));
    let lane = match narrowed {
        Err(Fail::Rejected(_)) => "rejected",
        Err(Fail::Denied(_)) => "denied",
        Err(Fail::Fatal(f)) if f.kind == errlanes::FatalKind::Exhausted => "exhausted",
        Err(Fail::Fatal(_)) => "fatal",
        Ok(()) => "ok",
    };
    assert_eq!(lane, "exhausted");

    let narrowed_fault = retry(|| rt.block_on(fake_find_by_id(true)));
    let lane = match narrowed_fault {
        Err(Fault::Denied(_)) => "denied",
        Err(Fault::Fatal(f)) if f.kind == errlanes::FatalKind::Exhausted => "exhausted",
        Err(Fault::Fatal(_)) => "fatal",
        Ok(()) => "ok",
    };
    assert_eq!(lane, "exhausted");

    assert!(instrumented().is_err());
    assert_eq!(
        lane_of_carrier(&HostFault::Transient(deadlock())),
        errlanes::Lane::Transient
    );

    // The box boundary: `Fault::classify` and `Lane::of` find the payload.
    let boxed: Box<dyn Error + Send + Sync> = Box::new(HostFault::Transient(deadlock()));
    assert!(Fault::classify(&*boxed).is_transient());
    assert_eq!(errlanes::Lane::of(&*boxed), Some(errlanes::Lane::Transient));

    // Narrowing.
    let denied: Result<(), PartyFault> = Err(PartyFault::Denied(Denied::new()));
    let narrowed: Result<(), Fault<lanes!(Transient, Fatal)>> = denied.narrow_denied();
    fn narrowed_into_host(r: Result<(), Fault<lanes!(Transient, Fatal)>>) -> Result<(), HostFault> {
        r?;
        Ok(())
    }
    let carried = narrowed_into_host(narrowed).unwrap_err();
    assert!(carried.is_fatal(), "a narrowed denial is a fatal");

    let transient: Result<(), HostFault> = Err(HostFault::Transient(deadlock()));
    let narrowed: Result<(), Fault<lanes!(Fatal)>> = transient.narrow_transient(2);
    assert!(narrowed.unwrap_err().is_fatal());

    let rejected: Result<(), CustomerError> =
        Err(CustomerError::Rejected(CustomerRejection::CustomerIsClosed));
    let narrowed = rejected.narrow_rejected();
    assert!(narrowed.unwrap_err().is_fatal());

    let rejected: Result<u8, CustomerError> =
        Err(CustomerError::Rejected(CustomerRejection::CustomerIsClosed));
    fn split(r: Result<u8, CustomerError>) -> Result<Option<u8>, Fault<lanes!(Transient, Fatal)>> {
        match r.rejected()? {
            Ok(v) => Ok(Some(v)),
            Err(CustomerRejection::CustomerIsClosed) => Ok(None),
            Err(CustomerRejection::Deposit(_)) => Ok(None),
        }
    }
    assert_eq!(split(rejected).unwrap(), None);

    let mapped: Result<u8, Fail<CustomerRejection, lanes!(Transient, Fatal)>> =
        Err::<u8, CustomerError>(CustomerError::Rejected(CustomerRejection::CustomerIsClosed))
            .map_rejected(|r| r);
    assert!(mapped.unwrap_err().as_rejected().is_some());

    assert!(rt.block_on(find_by_id_into_fail_view()).is_err());
}
