enum Source { Value { value: u32 }, Other }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct Destination { value: u32 }

fn main() {}
