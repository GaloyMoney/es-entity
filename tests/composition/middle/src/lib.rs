#[errlanes::compose(foreign::LaneParentConstraintViolation as Order)]
#[derive(Debug)]
pub enum WriteRejection {
    Local,
}
