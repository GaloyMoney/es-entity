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
    Err(f.widen::<CustomerRejection, errlanes::AllLanes>())?;
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
    // A settled failure has no transient arm at all, so the match names only
    // the lanes that can still occur. Exhaustion arrives in the fatal lane,
    // tagged `FatalKind::Exhausted` and carrying the last transient as source.
    let lane = match settled {
        Err(errlanes::Fail::Rejected(_)) => "rejected",
        Err(errlanes::Fail::Denied(_)) => "denied",
        Err(errlanes::Fail::Fatal(f)) if f.kind == errlanes::FatalKind::Exhausted => "exhausted",
        Err(errlanes::Fail::Fatal(_)) => "fatal",
        Ok(()) => "ok",
    };
    assert_eq!(lane, "exhausted");

    // `retry` over a bare read (`Fault`-returning, no `Failure` impl)
    // compiles via `Laned` and settles into `SettledFault`.
    let settled_fault = rt.block_on(errlanes::retry(&policy, || fake_find_by_id(true)));
    let lane = match settled_fault {
        Err(errlanes::Fault::Denied(_)) => "denied",
        Err(errlanes::Fault::Fatal(f)) if f.kind == errlanes::FatalKind::Exhausted => "exhausted",
        Err(errlanes::Fault::Fatal(_)) => "fatal",
        Ok(()) => "ok",
    };
    assert_eq!(lane, "exhausted");
}
