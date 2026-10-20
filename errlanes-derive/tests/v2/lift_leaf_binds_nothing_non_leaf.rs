enum Inner {
    Value(u8),
}

enum Source {
    Inner(Inner),
}

#[derive(errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
enum Destination {
    #[lift(Source::Inner { .. }(Inner::Value))]
    Value,
}

fn main() {}
