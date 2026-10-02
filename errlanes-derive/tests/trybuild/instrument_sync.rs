#[derive(Debug, errlanes::Rejection)]
enum MyRejection {
    Closed,
}

#[errlanes::instrument]
fn step() -> Result<(), errlanes::Fail<MyRejection>> {
    Err(MyRejection::Closed.into())
}

fn main() {
    assert!(step().is_err());
}
