enum Source {
    Held(u8),
}

struct Leaf {
    code: u32,
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Held { .. })]
    Gone(Leaf),
}

fn main() {}
