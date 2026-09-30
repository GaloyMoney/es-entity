//! A caller-owned retry loop hands back the settled profile, so a caller
//! that retries stops offering `Transient` to its own callers without
//! writing an adapter. The README covers the same ground.
use errlanes::{Fatal, Fault, Transient, TransientKind, lanes};

fn execute(
    mut inner: impl FnMut() -> Result<u64, Fault<lanes!(Transient, Fatal)>>,
) -> Result<u64, Fault<lanes!(Fatal)>> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match inner() {
            Ok(value) => return Ok(value),
            Err(failure) if failure.is_transient() && attempts < 3 => continue,
            Err(failure) => return Err(failure.settle(attempts)),
        }
    }
}

#[test]
fn retry_hands_back_the_settled_profile() {
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
