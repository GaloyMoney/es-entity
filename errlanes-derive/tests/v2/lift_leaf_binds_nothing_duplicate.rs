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

struct Leaf {
    a: u8,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Bound {
    #[lift(Source::Value { a, .. })]
    Built(Leaf),
    #[lift(Source::Value { .. })]
    Dropped,
}

fn main() {}
