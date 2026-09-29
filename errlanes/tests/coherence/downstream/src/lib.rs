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

/// Stand-in for a generated `find_by_id`: reads never reject, so the
/// signature is `Fault`, not `Fail<D>`.
async fn fake_find_by_id(fail: bool) -> Result<(), errlanes::Fault> {
    if fail {
        Err(errlanes::Transient::new(errlanes::TransientKind::Deadlock).into())
    } else {
        Ok(())
    }
}

/// `?` widens a `Fault` into a concrete `Fail<D>` via the blanket
/// `impl<D> From<Fault> for Fail<D>` — no `map_err`.
async fn find_by_id_into_fail_view() -> Result<(), Fail<CustomerRejection>> {
    fake_find_by_id(true).await?;
    Ok(())
}

/// `?` widens a `Fault` into a downstream `Failure` carrier — also no
/// `map_err`, via the derive's `From<Fault>` impl.
async fn find_by_id_into_carrier() -> Result<(), CustomerError> {
    fake_find_by_id(true).await?;
    Ok(())
}

/// Exercises the `?`-widening paths from Appendix A of the error-handling
/// research doc, plus `retry` over a `Failure` and a `Fault`-returning read,
/// and the exhaustive `Settled`/`SettledFault` matches with no `Transient`
/// arm.
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

    assert!(rt.block_on(find_by_id_into_fail_view()).is_err());
    assert!(rt.block_on(find_by_id_into_carrier()).is_err());

    let policy = errlanes::RetryPolicy::default();

    let settled = rt.block_on(errlanes::retry(&policy, || async {
        upstream_transient_call()
    }));
    let lane = match settled {
        Err(errlanes::Settled::Rejected(_)) => "rejected",
        Err(errlanes::Settled::Denied(_)) => "denied",
        Err(errlanes::Settled::Exhausted(_)) => "exhausted",
        Err(errlanes::Settled::Fatal(_)) => "fatal",
        Ok(()) => "ok",
    };
    assert_eq!(lane, "exhausted");

    // `retry` over a bare read (`Fault`-returning, no `Failure` impl)
    // compiles via `Laned` and settles into `SettledFault`.
    let settled_fault = rt.block_on(errlanes::retry(&policy, || fake_find_by_id(true)));
    let lane = match settled_fault {
        Err(errlanes::SettledFault::Denied(_)) => "denied",
        Err(errlanes::SettledFault::Exhausted(_)) => "exhausted",
        Err(errlanes::SettledFault::Fatal(_)) => "fatal",
        Ok(()) => "ok",
    };
    assert_eq!(lane, "exhausted");
}
