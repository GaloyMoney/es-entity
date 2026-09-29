#[derive(Debug, thiserror::Error, errlanes::Classify)]
enum MyError {
    #[error("rejected")]
    #[lane(rejected)]
    Rejected,
    #[error("denied")]
    #[lane(denied)]
    Denied,
    #[error(transparent)]
    #[lane(external)]
    Other(#[from] std::io::Error),
}

fn main() {
    use errlanes::Classify;
    let _ = MyError::Rejected.lane();
}
