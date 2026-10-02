// Appendix B, U2: a mixed wrapper (it has a rejected part, even if this
// particular value is not using it) never enters a `Fault` by `?` either —
// same reason as U1, generalised from a bare rejection to any `Classify`
// whose `Rejected` is not `Infallible`.
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[error("constraint violated: {0}")]
#[rejection(code = "CONSTRAINT")]
struct Constraint(&'static str);

#[derive(Debug, thiserror::Error, errlanes::Classify)]
enum DbWrite {
    #[error("constraint: {0}")]
    #[classify(delegate)]
    Constraint(Constraint),
    #[error("conflict: {0}")]
    #[classify(transient(OptimisticConflict))]
    Conflict(std::io::Error),
}

fn u2() -> Result<u8, errlanes::Fault<errlanes::lanes!(Transient, Fatal)>> {
    let r: Result<u8, DbWrite> = Err(DbWrite::Conflict(std::io::Error::other("x")));
    r?;
    Ok(0)
}

fn main() {}
