use errlanes::{Fail, Fault, Rejection, ResultExt, lanes};

#[errlanes::rejection]
#[derive(Debug, thiserror::Error, errlanes::Lift)]
pub enum Api {
    #[flatten(prefix = "Post")]
    Posting(middle::Posting),
    #[error("local")]
    Local,
}

pub fn propagate() -> Result<(), Fail<Api, lanes!(Transient, Fatal)>> {
    let child: Result<(), Fail<middle::Posting, lanes!(Fatal)>> =
        Err(Fail::Rejected(middle::limit()));
    child.widen()?;
    Ok(())
}
pub fn bare() -> Result<(), Fail<Api, lanes!(Fatal)>> {
    Err::<(), _>(middle::limit())?;
    Ok(())
}
pub fn fault() -> Result<(), Fail<Api, lanes!(Transient, Fatal)>> {
    let error: Fault<lanes!(Fatal)> = errlanes::Fatal::invariant("broken").into();
    Err::<(), _>(error)?;
    Ok(())
}
pub fn check() {
    let e = propagate().unwrap_err().rejected().unwrap();
    assert_eq!(Into::<&'static str>::into(e.code()), "VELOCITY_LIMIT");
    assert_eq!(e.level(), errlanes::Level::Warn);
    match e {
        Api::PostVelocityLimit(value) => assert_eq!(value.0, 42),
        _ => panic!("wrong mapping"),
    }
    assert!(matches!(
        bare(),
        Err(Fail::Rejected(Api::PostVelocityLimit(_)))
    ));
    assert_eq!(
        errlanes::lane_of(&fault().unwrap_err()),
        Some(errlanes::Lane::Fatal)
    );
    #[cfg(feature = "extra")]
    assert!(matches!(
        Api::from(middle::Posting::VelocityExtra),
        Api::PostVelocityExtra
    ));
}
