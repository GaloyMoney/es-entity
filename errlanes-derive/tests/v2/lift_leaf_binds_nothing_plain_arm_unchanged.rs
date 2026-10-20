enum Source {
    Held(u8),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Held)]
    Gone,
}

fn main() {}
