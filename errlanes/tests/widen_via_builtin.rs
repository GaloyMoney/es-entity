//! `ResultExt::widen_via_builtin`: every lane source lands in its own built-in
//! with no destination named, and nothing is lost on the way into a box.

use errlanes::{Fail, FatalKind, Fault, Lane, ResultExt, Transient, TransientKind, lanes};

use std::error::Error;

type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Debug, errlanes::Classify)]
#[classify(fatal(CorruptState))]
#[error("stored bytes do not decode")]
struct Undecodable(#[source] std::io::Error);

fn decode() -> Result<u8, Undecodable> {
    Err(Undecodable(std::io::Error::other("bad bytes")))
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "ACCOUNT_CLOSED")]
struct AccountClosed;

#[derive(Debug, errlanes::Carrier)]
enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

#[derive(Debug, errlanes::Carrier)]
enum PartyFault {
    Denied(errlanes::Denied),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

/// Every link of a boxed chain, outermost first, as `Display` text.
fn chain(e: &(dyn Error + 'static)) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = Some(e);
    while let Some(x) = cur {
        out.push(x.to_string());
        cur = x.source();
    }
    out
}

fn find<'a, T: Error + 'static>(e: &'a (dyn Error + 'static)) -> Option<&'a T> {
    let mut cur = Some(e);
    while let Some(x) = cur {
        if let Some(t) = x.downcast_ref::<T>() {
            return Some(t);
        }
        cur = x.source();
    }
    None
}

#[test]
fn the_built_in_is_the_sources_own_profile() {
    let _: Result<u8, Fault<lanes!(Fatal)>> = decode().widen_via_builtin();
    let _: Result<u8, Fault<lanes!(Transient, Fatal)>> =
        Err::<u8, _>(HostFault::Fatal(errlanes::Fatal::invariant("x"))).widen_via_builtin();
    let _: Result<u8, Fail<AccountClosed, errlanes::NoLanes>> =
        Err::<u8, _>(AccountClosed).widen_via_builtin();
    let _: Result<u8, Fault<lanes!(Transient)>> =
        Err::<u8, _>(Transient::new(TransientKind::Deadlock)).widen_via_builtin();
    let _: Result<u8, Fault<lanes!(Denied, Fatal)>> =
        Err::<u8, Fault<lanes!(Denied, Fatal)>>(errlanes::Fatal::invariant("x").into())
            .widen_via_builtin();
}

#[test]
fn a_wrapper_keeps_its_classification_and_its_whole_chain_across_a_box() {
    fn boundary() -> Result<u8, BoxError> {
        Ok(decode().widen_via_builtin()?)
    }
    let boxed = boundary().unwrap_err();

    // the lane and kind the wrapper declares
    match Fault::classify(&*boxed) {
        Fault::Fatal(f) => assert_eq!(f.kind, FatalKind::CorruptState),
        other => panic!("expected Fatal(CorruptState), got {other:?}"),
    }
    // the wrapper itself and the foreign error under it are still in the chain
    assert!(
        find::<Undecodable>(&*boxed).is_some(),
        "wrapper reachable by downcast"
    );
    assert_eq!(
        find::<std::io::Error>(&*boxed)
            .map(|e| e.to_string())
            .as_deref(),
        Some("bad bytes"),
        "the original foreign error reachable by downcast"
    );
    let links = chain(&*boxed);
    assert!(
        links.iter().any(|l| l == "stored bytes do not decode"),
        "{links:?}"
    );
    assert!(links.iter().any(|l| l == "bad bytes"), "{links:?}");
}

#[test]
fn boxing_the_wrapper_raw_is_what_loses_the_kind() {
    fn boundary() -> Result<u8, BoxError> {
        Ok(decode()?)
    }
    let boxed = boundary().unwrap_err();
    assert!(
        !matches!(Fault::classify(&*boxed), Fault::Fatal(f) if f.kind == FatalKind::CorruptState),
        "raw boxing skips the wrapper's classification; widen_via_builtin is the fix"
    );
}

#[test]
fn a_carrier_reaches_a_carrier_it_cannot_convert_into_directly() {
    // `HostFault -> PartyFault` has no `From` (no `from(..)` here); the
    // built-in is the hop every carrier absorbs.
    fn host() -> Result<u8, HostFault> {
        Err(HostFault::Transient(Transient::new(
            TransientKind::Deadlock,
        )))
    }
    fn party() -> Result<u8, PartyFault> {
        let v = host().widen_via_builtin()?;
        Ok(v)
    }
    assert!(matches!(party(), Err(PartyFault::Transient(t)) if t.kind == TransientKind::Deadlock));
}

/// Into a carrier, a `Fail`, or a box. Into a built-in `Fault` it is not
/// needed (and does not apply): a wrapper or carrier `?`s straight into any
/// `Fault` whose lanes fit, while `Fault -> Fault` is never `?` (`.widen()` is).
#[test]
fn into_fail_by_question_mark_and_into_fault_needs_no_hop() {
    fn into_fault() -> Result<u8, Fault> {
        decode()?;
        Ok(0)
    }
    fn into_fail() -> Result<u8, Fail<AccountClosed, lanes!(Transient, Fatal)>> {
        Err::<u8, _>(HostFault::Fatal(errlanes::Fatal::invariant("x"))).widen_via_builtin()?;
        Ok(0)
    }
    assert_eq!(into_fault().unwrap_err().lane(), Lane::Fatal);
    assert_eq!(into_fail().unwrap_err().lane(), Lane::Fatal);
}

/// What a boundary records (`error.code` = the kind, `error.level`,
/// `exception.message`) is identical whether the built-in is recorded directly
/// or recovered from the box. The one visible difference is the box's own
/// top-level `Display`: it is the payload's (`fatal(corrupt_state)`), as for
/// any `Fault`, and the wrapper's message moves one link down the chain.
#[test]
fn recorded_metadata_survives_the_box_unchanged() {
    use errlanes::Laned;

    let direct: Fault<lanes!(Fatal)> = decode().widen_via_builtin().unwrap_err();
    let boxed: BoxError = Box::new(decode().widen_via_builtin().unwrap_err());
    let recovered = Fault::classify(&*boxed);

    assert_eq!(recovered.lane(), direct.lane());
    assert_eq!(
        recovered.as_fatal().map(|f| f.kind),
        direct.as_fatal().map(|f| f.kind)
    );
    assert_eq!(recovered.message(), Laned::message(&direct));
    assert_eq!(
        recovered.message(),
        "fatal(corrupt_state): stored bytes do not decode: bad bytes"
    );
    assert_eq!(boxed.to_string(), "fatal(corrupt_state)");

    // Boxed raw, the text survives but the kind does not.
    let raw: BoxError = Box::new(decode().unwrap_err());
    assert_eq!(raw.to_string(), "stored bytes do not decode");
    assert!(matches!(Fault::classify(&*raw), Fault::Fatal(f) if f.kind == FatalKind::Dependency));
}

/// A rejection's code and level do not survive any box: `Fault::classify` has
/// no rejected arm, so it falls back to `Fatal(Dependency)`, through the
/// built-in or raw alike. A boundary that boxes must resolve its rejections
/// first (`narrow_rejected`, or handle them in the body).
#[test]
fn a_rejection_is_not_recoverable_from_a_box_either_way() {
    let via: BoxError = Box::new(Err::<(), _>(AccountClosed).widen_via_builtin().unwrap_err());
    let raw: BoxError = Box::new(AccountClosed);
    for boxed in [via, raw] {
        assert!(
            matches!(Fault::classify(&*boxed), Fault::Fatal(f) if f.kind == FatalKind::Dependency)
        );
    }
}
