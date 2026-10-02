#[derive(Debug, Clone, errlanes::Rejection)]
enum MyRejection {
    Closed,
}

#[derive(Debug, Clone, errlanes::Failure)]
struct MyError(errlanes::Fail<MyRejection>);

fn main() {
    let _: MyError = MyRejection::Closed.into();
}
