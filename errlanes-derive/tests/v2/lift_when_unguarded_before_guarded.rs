enum Source {
    Value(u8),
}

fn is_even(value: &u8) -> bool {
    value % 2 == 0
}

#[derive(errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
enum Destination {
    #[lift(Source::Value)]
    Any(u8),
    #[lift(Source::Value, when = is_even)]
    Even(u8),
}

fn main() {}
