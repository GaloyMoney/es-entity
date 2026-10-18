#[derive(Debug)]
enum Source {
    Value(u8),
}

#[derive(Debug, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
#[lift(Source::Value)]
enum Destination {
    Other,
}

fn main() {}
