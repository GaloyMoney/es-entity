#[derive(Debug, thiserror::Error, errlanes::Classify)]
enum MyError {
    #[error("oops")]
    Unclassified,
}

fn main() {}
