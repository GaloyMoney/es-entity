enum Source {
    Value(u8),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value { .. })]
    Dropped,
    #[lift(Source::Value)]
    Kept(u8),
}

fn main() {}
