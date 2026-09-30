#[lanes_runtime::rejection]
#[derive(Debug, thiserror::Error, lanes_runtime::Lift)]
pub enum Posting {
    #[flatten(prefix = "Velocity", rename(Disabled = VelocityUnavailable))]
    Velocity(renamed::Enforcement),
    #[error("batch too large")]
    Batch,
}
pub fn limit() -> Posting {
    renamed::Enforcement::Limit(renamed::domain::Limit(42)).into()
}
