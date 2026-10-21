enum Source {
    Held { a: u8 },
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Unit {
    #[lift(Source::Held { a, .. })]
    Gone,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum TwoFields {
    #[lift(Source::Held { a, .. })]
    Gone(u8, u8),
}

fn main() {}
