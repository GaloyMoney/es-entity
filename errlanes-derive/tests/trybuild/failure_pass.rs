#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
enum MyRejection {
    #[error("closed")]
    Closed,
}

#[derive(Debug, Clone, errlanes::Failure)]
struct MyError(errlanes::Fail<MyRejection>);

fn main() {
    let _: MyError = MyRejection::Closed.into();
}
