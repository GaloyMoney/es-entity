#[derive(Debug, errlanes::Rejection)]
enum Source {
    Value,
}

#[derive(Debug, errlanes::Rejection)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value)]
    Value,
}

fn main() {
    let _: Destination = Source::Value.into();
}
