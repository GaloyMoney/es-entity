enum Source {
    Value(u8),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Named {
    #[lift(Source::Value { x })]
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
enum Empty {
    #[lift(Source::Value {})]
    Value,
}

fn main() {}
