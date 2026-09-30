#[errlanes::rejection]
#[derive(Debug, thiserror::Error)]
pub enum WriteRejection {
    #[flatten(prefix = "Order")]
    Order(foreign::LaneParentConstraintViolation),
    #[error("local")]
    Local,
}
