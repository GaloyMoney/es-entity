// Appendix B, N: a wrapper enters bare `Fatal` only if `Fatal` is its only
// lane — a wrapper with more than one lane (here, `Transient` and `Fatal`)
// cannot narrow itself on the way in; that is an explicit decision, not an
// implicit one `?` should make.
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

fn n() -> Result<u8, errlanes::Fatal> {
    let r: Result<u8, DbWrite> = Err(DbWrite::Conflict(std::io::Error::other("x")));
    r?;
    Ok(0)
}

fn main() {}
