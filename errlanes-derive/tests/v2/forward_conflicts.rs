#[derive(Debug, errlanes::Rejection)]
pub enum Source {
    Value,
}

#[derive(Debug, errlanes::Rejection)]
enum Code {
    #[rejection(forward = Source::Value, code = "LOCAL")]
    Value,
}

#[derive(Debug, errlanes::Rejection)]
enum Level {
    #[rejection(forward = Source::Value, level = "warn")]
    Value,
}

#[derive(Debug, errlanes::Rejection)]
enum Delegate {
    #[rejection(forward = Source::Value, delegate)]
    Value(Source),
}

fn main() {}
