enum Source {
    Value(u8),
}

struct Leaf(u32);

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value { .. })]
    Value(Leaf),
}

fn main() {}
