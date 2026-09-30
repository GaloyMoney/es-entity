#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Source {
    #[error("value")]
    Value,
}

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[lift(Source)]
enum Destination {
    #[error("value")]
    #[lift(Source::Value)]
    Value,
}

fn main() {
    let _: Destination = Source::Value.into();
}
