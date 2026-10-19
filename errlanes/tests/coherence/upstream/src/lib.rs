//! The "errlanes stand-in" side of the two-crate coherence fixture: it exposes
//! a rejection and `Fault` / `Fail` signatures that a separate downstream
//! crate then converts, expands, narrows, and lifts.

use errlanes::{Fail, Fault, lanes};

#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
#[rejection(error = manual)]
pub enum DepositRejection {
    #[error("deposit account is frozen")]
    AccountFrozen,
    #[error("deposit exceeds the daily limit")]
    #[rejection(level = "warn")]
    DailyLimitExceeded,
}

/// Reads and host calls.
pub type HostFault = Fault<lanes!(Transient, Fatal)>;

/// A write that can reject.
pub type DepositError = Fail<DepositRejection, lanes!(Transient, Fatal)>;

pub fn frozen() -> DepositError {
    Fail::Rejected(DepositRejection::AccountFrozen)
}

pub fn transient() -> DepositError {
    Fail::Transient(errlanes::Transient::new(errlanes::TransientKind::Deadlock))
}

pub fn host_op() -> Result<u8, HostFault> {
    Ok(7)
}

pub fn host_transient() -> Result<u8, HostFault> {
    Err(Fault::Transient(errlanes::Transient::new(
        errlanes::TransientKind::Deadlock,
    )))
}
