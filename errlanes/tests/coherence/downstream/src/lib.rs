//! The "consumer" side of the two-crate coherence fixture. It absorbs the
//! upstream `Fault` / `Fail` signatures, so every claim here is a claim about
//! what a *downstream* crate may write (orphan rules, overlap against errlanes'
//! blankets), which a single-crate test cannot pin.

use std::error::Error;

use errlanes::{
    Denied, Exhausted, Fail, Fatal, Fault, Laned, ResultExt, Transient, TransientKind, lanes,
};
use upstream::{DepositError, HostFault};

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

type CustomerError = Fail<CustomerRejection, lanes!(Transient, Fatal)>;

/// A hand-rolled `thiserror` enum over the upstream built-in.
#[derive(Debug, thiserror::Error)]
pub enum HandRolled {
    #[error(transparent)]
    Host(#[from] HostFault),
}

// ---------------------------------------------------------------------------
// Effect bound: `E: Error + From<HostFault> + Send + Sync + 'static`
// ---------------------------------------------------------------------------

fn effect_error<E: Error + From<HostFault> + Send + Sync + 'static>() {}

fn in_effect<E: Error + From<HostFault>>() -> Result<u8, E> {
    let v = upstream::host_op()?;
    Ok(v)
}

fn effect_bound_holds_everywhere() {
    effect_error::<HostFault>();
    effect_error::<Fault<lanes!(Denied, Transient, Fatal)>>();
    effect_error::<Fail<CustomerRejection, lanes!(Transient, Fatal)>>();
    effect_error::<Fail<CustomerRejection, lanes!(Denied, Transient, Fatal)>>();
    effect_error::<HandRolled>();

    assert_eq!(in_effect::<HostFault>().unwrap(), 7);
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

fn into_host_from_exhausted() -> Result<(), HostFault> {
    Err(exhausted())?;
    Ok(())
}

fn into_wide_from_denied() -> Result<(), Fault<lanes!(Denied, Transient, Fatal)>> {
    Err(Denied::new())?;
    Ok(())
}

fn into_customer_from_rejection() -> Result<(), CustomerError> {
    Err(CustomerRejection::CustomerIsClosed)?;
    Ok(())
}

fn into_customer_from_upstream_rejection() -> Result<(), CustomerError> {
    Err(upstream::DepositRejection::AccountFrozen)?;
    Ok(())
}

fn into_customer_from_upstream_fail() -> Result<(), CustomerError> {
    Err::<(), _>(upstream::frozen()).lift()?;
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
    upstream::host_transient()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Outbound `?` / widen
// ---------------------------------------------------------------------------

fn host_into_wide_fault() -> Result<(), Fault<lanes!(Denied, Transient, Fatal)>> {
    upstream::host_transient()?;
    Ok(())
}

fn host_into_all_lanes_fault() -> Result<(), Fault> {
    upstream::host_op()?;
    Ok(())
}

fn customer_into_wide_fail() -> Result<(), Fail<CustomerRejection>> {
    into_customer_from_host()?;
    Ok(())
}

fn fault_only_wrapper_into_bare_fatal() -> Result<(), Fatal> {
    Err(stored())?;
    Ok(())
}

fn expand_tail_position() -> Result<(), Fault> {
    upstream::host_transient().map(|_| ()).map_err(Into::into)
}

fn partial_lift_from_upstream_fail() -> Result<(), Fail<FrozenOnly, lanes!(Transient, Fatal)>> {
    let r: Result<(), DepositError> = Err(Fail::Rejected(
        upstream::DepositRejection::DailyLimitExceeded,
    ));
    r.lift()?;
    Ok(())
}

fn partial_lift_maps_a_handled_case() -> Result<(), Fail<FrozenOnly, lanes!(Transient, Fatal)>> {
    Err::<(), _>(upstream::frozen()).lift()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Generic code
// ---------------------------------------------------------------------------

/// Hand-rolled retry loop generic over anything `Laned`, with no retry
/// machinery to depend on.
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
    upstream::host_transient()
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

/// Exercises every positive claim of the two-crate coherence fixture.
pub fn smoke() {
    effect_bound_holds_everywhere();

    // Inbound.
    assert!(into_host_from_transient().unwrap_err().is_transient());
    assert!(into_host_from_fatal().unwrap_err().is_fatal());
    assert!(into_host_from_wrapper().unwrap_err().is_fatal());
    assert!(into_host_from_narrower_fault().unwrap_err().is_fatal());
    assert!(into_host_from_exhausted().unwrap_err().is_fatal());
    assert!(into_wide_from_denied().unwrap_err().is_denied());
    assert!(
        into_customer_from_rejection()
            .unwrap_err()
            .as_rejected()
            .is_some()
    );
    assert!(
        into_customer_from_upstream_rejection()
            .unwrap_err()
            .as_rejected()
            .is_some()
    );
    assert!(
        into_customer_from_upstream_fail()
            .unwrap_err()
            .as_rejected()
            .is_some()
    );
    assert!(into_customer_from_wrapper().unwrap_err().is_fatal());
    assert!(into_customer_from_payload().unwrap_err().is_transient());
    assert!(into_customer_from_narrower_fail().unwrap_err().is_fatal());
    assert!(into_customer_from_host().unwrap_err().is_transient());

    // Outbound / widen.
    assert!(host_into_wide_fault().unwrap_err().is_transient());
    assert!(host_into_all_lanes_fault().is_ok());
    assert!(customer_into_wide_fail().unwrap_err().is_transient());
    assert!(fault_only_wrapper_into_bare_fatal().is_err());
    assert!(expand_tail_position().unwrap_err().is_transient());
    assert!(partial_lift_from_upstream_fail().unwrap_err().is_fatal());
    assert!(
        partial_lift_maps_a_handled_case()
            .unwrap_err()
            .as_rejected()
            .is_some()
    );

    // Generic code.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    assert!(rt.block_on(find_by_id_into_fail_view()).is_err());

    let narrowed = retry(upstream::host_transient);
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

    assert!(instrumented().is_err());

    // The box boundary: `Fault::classify` and `Lane::of` find the payload.
    let boxed: Box<dyn Error + Send + Sync> = Box::new(HostFault::Transient(deadlock()));
    assert!(Fault::classify(&*boxed).is_transient());
    assert_eq!(errlanes::Lane::of(&*boxed), Some(errlanes::Lane::Transient));

    // Narrowing.
    let denied: Result<(), Fault<lanes!(Denied, Transient, Fatal)>> =
        Err(Fault::Denied(Denied::new()));
    let narrowed: Result<(), HostFault> = denied.narrow_denied();
    assert!(
        narrowed.unwrap_err().is_fatal(),
        "a narrowed denial is a fatal"
    );

    let transient: Result<(), HostFault> = Err(Fault::Transient(deadlock()));
    let narrowed: Result<(), Fault<lanes!(Fatal)>> = transient.narrow_transient(2);
    assert!(narrowed.unwrap_err().is_fatal());

    let rejected: Result<(), CustomerError> =
        Err(Fail::Rejected(CustomerRejection::CustomerIsClosed));
    let narrowed = rejected.narrow_rejected();
    assert!(narrowed.unwrap_err().is_fatal());

    let rejected: Result<u8, CustomerError> =
        Err(Fail::Rejected(CustomerRejection::CustomerIsClosed));
    fn split(r: Result<u8, CustomerError>) -> Result<Option<u8>, HostFault> {
        match r.rejected()? {
            Ok(v) => Ok(Some(v)),
            Err(CustomerRejection::CustomerIsClosed) => Ok(None),
            Err(CustomerRejection::Deposit(_)) => Ok(None),
        }
    }
    assert_eq!(split(rejected).unwrap(), None);

    let mapped: Result<u8, CustomerError> =
        Err::<u8, CustomerError>(Fail::Rejected(CustomerRejection::CustomerIsClosed))
            .map_rejected(|r| r);
    assert!(mapped.unwrap_err().as_rejected().is_some());
}
