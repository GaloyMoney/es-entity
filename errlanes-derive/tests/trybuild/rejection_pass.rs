#[derive(Debug, Clone, errlanes::Rejection)]
enum MyRejection {
    Closed,
    #[rejection(level = "warn")]
    LimitExceeded,
}

fn main() {
    use errlanes::Rejection;
    let code: &'static str = MyRejection::Closed.code().into();
    assert_eq!(code, "CLOSED");
}
