enum Source {
    Held { a: u8, b: u8 },
}

struct Leaf {
    a: u8,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Held { a })]
    Gone(Leaf),
}

fn main() {}
