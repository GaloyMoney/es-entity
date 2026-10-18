#[derive(Debug)]
enum Source {
    Value,
}

#[derive(Debug, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
#[lift(Source::Value, to = Missing)]
enum Destination {
    Present,
}

fn main() {}
