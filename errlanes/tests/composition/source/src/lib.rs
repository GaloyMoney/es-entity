pub mod domain {
    #[derive(Debug, thiserror::Error)]
    #[error("limit {0}")]
    pub struct Limit(pub u64);

    #[derive(Debug, errlanes::Rejection)]
    #[rejection(code_prefix = "VELOCITY_")]
    pub enum Enforcement {
        #[error("limit: {0}")]
        #[rejection(level = "warn", from)]
        Limit(Limit),
        #[error("disabled")]
        Disabled,
        #[error("range {min}..{max}")]
        Range { min: u64, max: u64 },
        #[cfg(feature = "extra")]
        #[error("extra")]
        Extra,
    }
}
pub use domain::{Enforcement, EnforcementSchema};
