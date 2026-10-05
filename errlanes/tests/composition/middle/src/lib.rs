#[lanes_runtime::compose(renamed::Enforcement as Velocity)]
#[derive(Debug)]
pub enum Posting {
    #[error("batch too large")]
    Batch,
}
pub fn limit() -> Posting {
    renamed::Enforcement::Limit(renamed::domain::Limit(42)).into()
}

#[lanes_runtime::compose(renamed::Enforcement, renamed::Secondary)]
#[derive(Debug)]
pub enum Combined {
    #[compose(merge)]
    #[error("canonical limit {0}")]
    #[rejection(code = "VELOCITY_LIMIT", level = "warn", from)]
    Limit(renamed::domain::Limit),
}

pub fn combined_limit() -> Combined {
    renamed::Secondary::Limit(renamed::domain::Limit(43)).into()
}
