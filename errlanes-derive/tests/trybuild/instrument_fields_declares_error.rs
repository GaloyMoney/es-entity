#[derive(Debug, errlanes::Rejection)]
enum MyRejection {
    Closed,
}

#[errlanes::instrument(fields(error = tracing::field::Empty))]
fn step() -> Result<(), errlanes::Fail<MyRejection>> {
    Err(MyRejection::Closed.into())
}

fn main() {
    assert!(step().is_err());
}
