use errlanes::{Fail, Fault, Rejection, ResultExt, lanes};

#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
pub enum Api {
    #[compose(flatten)]
    Post(middle::Posting),
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
    let disabled = Api::from(middle::Posting::VelocityDisabled);
    assert!(matches!(disabled, Api::PostVelocityDisabled));
    assert_eq!(
        Into::<&'static str>::into(disabled.code()),
        "VELOCITY_DISABLED"
    );
    let range = Api::from(middle::Posting::VelocityRange { min: 1, max: 9 });
    assert!(matches!(range, Api::PostVelocityRange { min: 1, max: 9 }));
    assert_eq!(range.to_string(), "range 1..9");
    let with_source = Api::from(middle::limit());
    assert_eq!(
        std::error::Error::source(&with_source).unwrap().to_string(),
        "limit 42"
    );
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
