enum Source {
    Unit,
    Tuple(u8, u8),
    Named { value: u8 },
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Unit, into)]
    Unit(u64),
    #[lift(Source::Tuple, into)]
    Tuple(u64),
    #[lift(Source::Named, into)]
    Named(u64),
}

fn main() {}
