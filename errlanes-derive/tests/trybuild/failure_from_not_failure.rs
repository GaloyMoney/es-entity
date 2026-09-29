#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
enum MyRejection {
    #[error("closed")]
    Closed,
}

#[derive(Debug, Clone, errlanes::Failure)]
#[failure(from(NotAFailure))]
struct MyError(errlanes::Fail<MyRejection>);

#[derive(Debug)]
struct NotAFailure;

fn main() {}
