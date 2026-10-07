#[derive(Debug, errlanes::Rejection)]
pub enum Source {
    Value,
}

#[derive(Debug, errlanes::Rejection)]
enum Code {
    #[rejection(code_and_level_from = Source::Value, code = "LOCAL")]
    Value,
}

#[derive(Debug, errlanes::Rejection)]
enum Level {
    #[rejection(code_and_level_from = Source::Value, level = "warn")]
    Value,
}

#[derive(Debug, errlanes::Rejection)]
enum Delegate {
    #[rejection(code_and_level_from = Source::Value, delegate)]
    Value(Source),
}

fn main() {}
