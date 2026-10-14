enum Source {
    Value(u8),
}

fn is_even(value: &u8) -> bool {
    value % 2 == 0
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value, when = is_even)]
    Even(u8),
}

fn main() {}
