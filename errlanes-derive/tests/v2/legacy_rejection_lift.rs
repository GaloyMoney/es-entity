#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[rejection(lift(Source))]
enum Destination {
    #[error("value")]
    #[rejection(key = Key::Value, via = Source)]
    Value,
}

fn main() {}
