#[lanes_runtime::compose]
#[derive(Debug, thiserror::Error)]
pub enum Posting {
    #[compose(flatten)]
    Velocity(renamed::Enforcement),
    #[error("batch too large")]
    Batch,
}
pub fn limit() -> Posting {
    renamed::Enforcement::Limit(renamed::domain::Limit(42)).into()
}
