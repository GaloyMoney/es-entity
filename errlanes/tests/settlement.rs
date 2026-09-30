//! `retry` owns the loop and hands back the settled profile, so a caller that
//! retries stops offering `Transient` to its own callers without writing an
//! adapter. The README covers the same ground without the `tokio` feature.
use errlanes::{Fatal, Fault, RetryPolicy, Transient, TransientKind, lanes, retry_with};

async fn execute(
    mut inner: impl FnMut() -> Result<u64, Fault<lanes!(Transient, Fatal)>>,
) -> Result<u64, Fault<lanes!(Fatal)>> {
    let policy = RetryPolicy::default();
    retry_with(
        &policy,
        || {
            let attempt = inner();
            async move { attempt }
        },
        |_| async {},
    )
    .await
    // Nothing here: `retry` already returns `Fault<Settled<lanes!(Transient,
    // Fatal)>>`, which *is* `Fault<lanes!(Fatal)>`. No adapter, no `map_err`.
}

#[tokio::test]
async fn retry_hands_back_the_settled_profile() {
    let mut attempts = 0;
    let value = execute(|| {
        attempts += 1;
        if attempts == 1 {
            Err(Transient::new(TransientKind::Deadlock).into())
        } else {
            Ok(42)
        }
    })
    .await
    .unwrap();
    assert_eq!(value, 42);
    assert_eq!(attempts, 2);

    // A fatal outcome stays fatal, and the caller can destructure it with a
    // single irrefutable `let` because Fatal is the only lane left.
    let err = execute(|| Err(Fatal::invariant("broken state").into()))
        .await
        .unwrap_err();
    let Fault::Fatal(fatal) = err;
    assert_eq!(fatal.context.as_deref(), Some("broken state"));

    // Exhaustion becomes Fatal(Exhausted), with the last transient as source.
    let err = execute(|| Err(Transient::new(TransientKind::Deadlock).into()))
        .await
        .unwrap_err();
    let Fault::Fatal(fatal) = err;
    assert_eq!(fatal.kind, errlanes::FatalKind::Exhausted);
    assert!(
        std::error::Error::source(&fatal)
            .unwrap()
            .is::<errlanes::Exhausted>()
    );
}
