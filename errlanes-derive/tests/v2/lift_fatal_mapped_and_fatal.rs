enum Source {
    A,
    B(Inner),
}

enum Inner {
    X,
    Y,
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(A))]
enum Mapped {
    #[lift(Source::A)]
    A,
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(B(Inner::X)))]
enum MappedAncestor {
    #[lift(Source::A)]
    A,
    #[lift(Source::B)]
    B(Inner),
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(A, A))]
enum ListedTwice {
    #[lift(Source::B(Inner::X))]
    X,
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(B, B(Inner::Y)))]
enum ListedUnderListed {
    #[lift(Source::A)]
    A,
}

#[derive(errlanes::Lift)]
#[lift(Source, variant = A, fatal(A))]
struct StructMapped {}

fn main() {}
