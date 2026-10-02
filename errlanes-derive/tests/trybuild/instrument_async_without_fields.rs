#[derive(Debug, errlanes::Rejection)]
enum MyRejection {
    Closed,
}

#[errlanes::instrument(name = "job.step")]
async fn step() -> Result<(), errlanes::Fail<MyRejection>> {
    Err(MyRejection::Closed.into())
}

fn main() {
    let _ = step;
}
