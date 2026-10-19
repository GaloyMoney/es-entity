enum Source {
    A(Inner),
    Z,
}

enum Inner {
    X,
    Y,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Nested {
    #[lift(Source::A(Inner::X))]
    X,
}

fn is_even(value: &u8) -> bool {
    value % 2 == 0
}

enum Guarded {
    Value(u8),
}

#[derive(errlanes::Lift)]
#[lift(Guarded)]
enum Destination {
    #[lift(Guarded::Value, when = is_even)]
    Even(u8),
}

fn main() {}
