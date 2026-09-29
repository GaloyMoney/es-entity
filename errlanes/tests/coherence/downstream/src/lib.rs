use errlanes::Fail;

#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
pub enum CustomerRejection {
    #[error("customer is closed")]
    CustomerIsClosed,
    #[error(transparent)]
    Deposit(#[from] upstream::DepositRejection),
}

#[derive(Debug, Clone, errlanes::Failure)]
#[failure(from(upstream::DepositError))]
pub struct CustomerError(pub Fail<CustomerRejection>);

fn from_own_rejection() -> Result<(), CustomerError> {
    Err(CustomerRejection::CustomerIsClosed)?;
    Ok(())
}

fn from_own_fail_view() -> Result<(), CustomerError> {
    Err(Fail::Rejected(CustomerRejection::CustomerIsClosed))?;
    Ok(())
}

fn from_upstream_fail_view() -> Result<(), CustomerError> {
    let f: Fail<upstream::DepositRejection> =
        Fail::Rejected(upstream::DepositRejection::AccountFrozen);
    Err(f.widen::<CustomerRejection>())?;
    Ok(())
}

fn from_upstream_carrier() -> Result<(), CustomerError> {
    Err(upstream::frozen())?;
    Ok(())
}

fn from_bare_transient() -> Result<(), CustomerError> {
    Err(errlanes::Transient::new(errlanes::TransientKind::Deadlock))?;
    Ok(())
}

fn upstream_transient_call() -> Result<(), CustomerError> {
    Err(upstream::transient())?;
    Ok(())
}

/// Exercises the `?`-widening paths from Appendix A of the error-handling
/// research doc, plus `retry` over a `Failure` and an exhaustive `Settled`
/// match with no `Transient` arm.
pub fn smoke() {
    assert!(from_own_rejection().is_err());
    assert!(from_own_fail_view().is_err());
    assert!(from_upstream_fail_view().is_err());
    assert!(from_upstream_carrier().is_err());
    assert!(from_bare_transient().is_err());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let settled = rt.block_on(errlanes::retry(
        errlanes::RetryPolicy::default(),
        || async { upstream_transient_call() },
    ));
    let lane = match settled {
        Err(errlanes::Settled::Rejected(_)) => "rejected",
        Err(errlanes::Settled::Denied(_)) => "denied",
        Err(errlanes::Settled::Exhausted(_)) => "exhausted",
        Err(errlanes::Settled::Fatal(_)) => "fatal",
        Ok(()) => "ok",
    };
    assert_eq!(lane, "exhausted");
}
