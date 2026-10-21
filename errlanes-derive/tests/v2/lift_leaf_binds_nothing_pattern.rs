enum Source {
    Value(u8),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Empty {
    #[lift(Source::Value {})]
    Value,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Trailing {
    #[lift(Source::Value { .., })]
    Value,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum RestFirst {
    #[lift(Source::Value { .., a })]
    Value,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Renamed {
    #[lift(Source::Value { a: b })]
    Value,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Twice {
    #[lift(Source::Value { a, a })]
    Value,
}

fn main() {}
