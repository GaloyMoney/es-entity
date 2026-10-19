struct Source(u8);

#[derive(errlanes::Lift)]
#[lift(Source, fatal(A))]
enum Destination {
    #[lift(Source, into)]
    Value(u64),
}

fn main() {}
