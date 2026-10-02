#[derive(Debug, errlanes::Rejection)]
#[rejection(lift(Source))]
enum Destination {
    #[rejection(key = Key::Value, via = Source)]
    Value,
}

fn main() {}
