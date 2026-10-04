enum Source {
    Value(u8),
    Other,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value, into)]
    Value(u64),
}

fn main() {}
