use std::error::Error;

use errlanes::{Classify, Denied, Fail, FatalKind, Fault, Lane, lanes};

const SENTINEL: &str = "sentinel-credential-9f3a";

#[derive(Debug, errlanes::Classify)]
#[classify(denied)]
#[error("provider refused our credentials for {tenant}")]
struct ProviderRefused {
    tenant: &'static str,
    #[source]
    cause: std::io::Error,
}

fn refused() -> ProviderRefused {
    ProviderRefused {
        tenant: "acme",
        cause: std::io::Error::other(SENTINEL),
    }
}

fn chain_has<T: Error + 'static>(e: &(dyn Error + 'static)) -> bool {
    let mut cur = Some(e);
    while let Some(x) = cur {
        if x.is::<T>() {
            return true;
        }
        cur = x.source();
    }
    false
}

#[test]
fn static_denied_keeps_the_whole_wrapper_and_its_native_source() {
    let Fail::Denied(d) = refused().classify();
    let wrapper = d
        .source()
        .and_then(|s| s.downcast_ref::<ProviderRefused>())
        .expect("the wrapper is the Denied's source");
    assert_eq!(wrapper.tenant, "acme");
    assert!(chain_has::<std::io::Error>(&d));
}

#[test]
fn static_denied_survives_narrowing_with_its_message() {
    let Fail::Denied(d) = refused().classify();
    let narrowed = Fault::<lanes!(Denied, Fatal)>::Denied(d).narrow_denied();
    let fatal = narrowed.as_fatal().expect("narrowed to fatal");
    assert_eq!(fatal.kind, FatalKind::Denied);
    assert!(chain_has::<ProviderRefused>(fatal));
    assert!(chain_has::<std::io::Error>(fatal));
    let message = narrowed.message();
    assert!(message.contains("provider refused our credentials for acme"));
    assert!(message.contains(SENTINEL), "{message}");
    assert_eq!(Lane::of(fatal), Some(Lane::Fatal));
}

#[test]
fn ordinary_denied_rendering_never_shows_the_retained_diagnostics() {
    let denied = Denied::new()
        .with_action("read")
        .with_object("ledger")
        .with_context(SENTINEL)
        .with_source(std::io::Error::other(SENTINEL));
    assert_eq!(denied.to_string(), "denied: read on ledger");
    assert_eq!(denied.action.as_deref(), Some("read"));
    assert_eq!(denied.object.as_deref(), Some("ledger"));

    let fault: Fault<lanes!(Denied, Fatal)> = denied.clone().into();
    assert_eq!(fault.to_string(), "denied: read on ledger");
    assert_eq!(fault.message(), "denied: read on ledger");
    let via_static: Fault<lanes!(Denied)> = match refused().classify() {
        Fail::Denied(d) => d.into(),
    };
    assert_eq!(via_static.message(), "denied");
    assert_eq!(
        Fault::<lanes!(Denied)>::Denied(Denied::default()).message(),
        "denied"
    );
}

#[test]
fn clone_and_expand_keep_context_and_source() {
    let denied = Denied::new()
        .with_context("ctx-breadcrumb")
        .with_source(std::io::Error::other(SENTINEL));
    let narrow_profile: Fault<lanes!(Denied)> = denied.into();
    let cloned = narrow_profile.clone();
    let converted: Fault<lanes!(Denied, Transient, Fatal)> = cloned.into();
    let Fault::Denied(d) = &converted else {
        panic!("still denied");
    };
    assert_eq!(d.context(), Some("ctx-breadcrumb"));
    assert!(chain_has::<std::io::Error>(d));

    let narrowed = converted.narrow_denied();
    let message = narrowed.message();
    assert_eq!(
        message,
        format!("fatal(denied): ctx-breadcrumb: denied: {SENTINEL}")
    );
    let fatal = narrowed.as_fatal().unwrap();
    assert_eq!(fatal.context.as_deref(), Some("ctx-breadcrumb"));
}

#[test]
fn erased_boundary_recovers_a_denied_with_its_diagnostics() {
    let denied = Denied::new()
        .with_context("ctx-breadcrumb")
        .with_source(std::io::Error::other(SENTINEL));
    let boxed: Box<dyn Error + Send + Sync> = Box::new(Fault::<lanes!(Denied)>::Denied(denied));
    let recovered = Fault::classify(boxed.as_ref());
    let Fault::Denied(d) = &recovered else {
        panic!("denied survives the box");
    };
    assert_eq!(d.context(), Some("ctx-breadcrumb"));
    assert!(chain_has::<std::io::Error>(d));
}
