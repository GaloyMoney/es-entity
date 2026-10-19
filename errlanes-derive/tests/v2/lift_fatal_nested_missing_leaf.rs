enum Source {
    A,
    B(Inner),
}

enum Inner {
    X,
    Y,
    Z,
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(A, B(Inner::X)))]
enum Destination {
    #[lift(Source::B(Inner::Y))]
    Y,
}

fn main() {}
