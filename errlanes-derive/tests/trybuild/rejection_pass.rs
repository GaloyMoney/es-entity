#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
enum MyRejection {
    #[error("closed")]
    Closed,
    #[error("limit exceeded")]
    #[rejection(level = "warn")]
    LimitExceeded,
}

fn main() {
    use errlanes::Rejection;
    let code: &'static str = MyRejection::Closed.code().into();
    assert_eq!(code, "CLOSED");
}
