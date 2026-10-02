// Appendix B, U1: a rejection never enters a `Fault` by `?` — nothing about
// it is a fault, so there is nothing for `Fault` to lift it into.
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[error("invalid amount")]
#[rejection(code = "INVALID_AMOUNT")]
struct Validation;

fn u1() -> Result<u8, errlanes::Fault<errlanes::lanes!(Transient, Fatal)>> {
    let r: Result<u8, Validation> = Err(Validation);
    r?;
    Ok(0)
}

fn main() {}
