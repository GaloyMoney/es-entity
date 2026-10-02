#[derive(Debug, Clone, errlanes::Rejection)]
enum MyRejection {
    Closed,
}

#[derive(Debug, Clone, errlanes::Failure)]
#[failure(from(NotAFailure))]
struct MyError(errlanes::Fail<MyRejection>);

#[derive(Debug)]
struct NotAFailure;

fn main() {}
