use errlanes::{
    Denied, Fail, Fault, WithDenied, WithFatal, WithTransient, WithoutDenied, WithoutTransient,
    lanes,
};

type MyFault = Fault<lanes!(Transient, Fatal)>;
type MyFail = Fail<Nope, lanes!(Transient, Fatal)>;

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "nope")]
#[error("nope")]
pub struct Nope;

#[derive(Debug, errlanes::Carrier)]
pub enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn base() -> Result<(), MyFault> {
    Ok(())
}
fn host() -> Result<(), HostFault> {
    Ok(())
}

fn may_deny(deny: bool) -> Result<(), WithDenied<MyFault>> {
    base()?;
    if deny {
        return Err(Fault::Denied(Denied::new()));
    }
    Ok(())
}
fn host_may_deny() -> Result<(), WithDenied<HostFault>> {
    host()?;
    Err(Denied::new().into())
}
fn fail_may_deny() -> Result<(), WithDenied<MyFail>> {
    Err(Fail::Rejected(Nope))
}

fn no_transient(f: WithoutTransient<MyFault>) -> Fault<lanes!(Fatal)> {
    f
}
fn round_trip(f: WithoutDenied<WithDenied<MyFault>>) -> MyFault {
    f
}
fn add_transient(f: WithTransient<Fault<lanes!(Fatal)>>) -> MyFault {
    f
}
fn add_fatal(f: WithFatal<Fault<lanes!(Transient)>>) -> MyFault {
    f
}
fn on_profile(f: Fault<WithoutTransient<lanes!(Transient, Fatal)>>) -> Fault<lanes!(Fatal)> {
    f
}
fn on_carrier(f: WithoutTransient<HostFault>) -> Fault<lanes!(Fatal)> {
    f
}
fn exhaustive(f: WithDenied<MyFault>) -> &'static str {
    match f {
        Fault::Denied(_) => "denied",
        Fault::Transient(_) => "transient",
        Fault::Fatal(_) => "fatal",
    }
}
fn generic<E: errlanes::Reshape>(e: E::WithDenied) -> WithDenied<E> {
    e
}

#[test]
fn reshape_aliases() {
    may_deny(false).unwrap();
    assert!(may_deny(true).unwrap_err().is_denied());
    assert!(host_may_deny().unwrap_err().is_denied());
    assert!(fail_may_deny().is_err());
    assert_eq!(exhaustive(Fault::Denied(Denied::new())), "denied");
    let _ = (
        no_transient,
        round_trip,
        add_transient,
        add_fatal,
        on_profile,
        on_carrier,
        generic::<MyFault>,
    );
}
