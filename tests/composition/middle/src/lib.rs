#[errlanes::rejection]
#[derive(Debug, thiserror::Error, errlanes::Lift)]
pub enum WriteRejection {
    #[flatten(prefix = "Order")]
    Order(foreign::LaneParentConstraintViolation),
    #[error("local")]
    Local,
}
