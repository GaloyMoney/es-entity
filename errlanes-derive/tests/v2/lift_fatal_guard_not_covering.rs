enum Source {
    A(u8),
    B,
}

fn is_even(value: &u8) -> bool {
    value % 2 == 0
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(B))]
enum Destination {
    #[lift(Source::A, when = is_even)]
    Even(u8),
}

fn main() {}
