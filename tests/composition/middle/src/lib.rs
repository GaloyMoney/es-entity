#[errlanes::compose]
#[derive(Debug)]
pub enum WriteRejection {
    #[compose(flatten)]
    Order(foreign::LaneParentConstraintViolation),
    Local,
}
