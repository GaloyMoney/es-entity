use errlanes::Fail;

#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
#[rejection(error = manual)]
pub enum DepositRejection {
    #[error("deposit account is frozen")]
    AccountFrozen,
    #[error("deposit exceeds the daily limit")]
    #[rejection(level = "warn")]
    DailyLimitExceeded,
}

#[derive(Debug, Clone, errlanes::Failure)]
pub struct DepositError(pub Fail<DepositRejection>);

pub fn frozen() -> DepositError {
    DepositError(Fail::Rejected(DepositRejection::AccountFrozen))
}

pub fn transient() -> DepositError {
    DepositError(Fail::from(errlanes::Transient::new(
        errlanes::TransientKind::Deadlock,
    )))
}
