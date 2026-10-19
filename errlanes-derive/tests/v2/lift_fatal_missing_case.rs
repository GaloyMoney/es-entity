enum Source {
    A,
    B,
    C,
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(B))]
enum Destination {
    #[lift(Source::A)]
    A,
}

fn main() {}
