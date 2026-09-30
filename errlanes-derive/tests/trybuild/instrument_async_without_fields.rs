#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum MyRejection {
    #[error("closed")]
    Closed,
}

#[errlanes::instrument(name = "job.step")]
async fn step() -> Result<(), errlanes::Fail<MyRejection>> {
    Err(MyRejection::Closed.into())
}

fn main() {
    let _ = step;
}
