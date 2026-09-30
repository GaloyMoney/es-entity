#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
pub enum WriteRejection {
    #[compose(flatten)]
    Order(foreign::LaneParentConstraintViolation),
    #[error("local")]
    Local,
}
