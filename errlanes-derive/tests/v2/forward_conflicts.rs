#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Source {
    #[error("value")]
    Value,
}

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Code {
    #[error("value")]
    #[rejection(forward = Source::Value, code = "LOCAL")]
    Value,
}

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Level {
    #[error("value")]
    #[rejection(forward = Source::Value, level = "warn")]
    Value,
}

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Delegate {
    #[error("value {0}")]
    #[rejection(forward = Source::Value, delegate)]
    Value(Source),
}

fn main() {}
