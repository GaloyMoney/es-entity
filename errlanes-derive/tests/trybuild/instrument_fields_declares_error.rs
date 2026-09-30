#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum MyRejection {
    #[error("closed")]
    Closed,
}

#[errlanes::instrument(fields(error = tracing::field::Empty))]
fn step() -> Result<(), errlanes::Fail<MyRejection>> {
    Err(MyRejection::Closed.into())
}

fn main() {
    assert!(step().is_err());
}
