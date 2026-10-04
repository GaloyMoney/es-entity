enum Source {
    Value(u8),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Unit {
    #[lift(Source::Value, into)]
    Value,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Tuple {
    #[lift(Source::Value, into)]
    Value(u64, u64),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Named {
    #[lift(Source::Value, into)]
    Value { first: u64, second: u64 },
}

fn main() {}
