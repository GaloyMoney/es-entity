// Appendix B, U2: a mixed wrapper (it has a rejected part, even if this
// particular value is not using it) never enters a `Fault` by `?` either —
// same reason as U1, generalised from a bare rejection to any `Classify`
// whose `Rejected` is not `Infallible`.
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "CONSTRAINT")]
struct Constraint(&'static str);

#[derive(Debug, errlanes::Classify)]
enum DbWrite {
    #[classify(delegate)]
    Constraint(Constraint),
    #[classify(transient(OptimisticConflict))]
    Conflict(#[source] std::io::Error),
}

fn u2() -> Result<u8, errlanes::Fault<errlanes::lanes!(Transient, Fatal)>> {
    let r: Result<u8, DbWrite> = Err(DbWrite::Conflict(std::io::Error::other("x")));
    r?;
    Ok(0)
}

fn main() {}
