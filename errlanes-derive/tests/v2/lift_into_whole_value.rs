struct Source(u8);

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source, into)]
    Value(u64),
}

fn main() {}
