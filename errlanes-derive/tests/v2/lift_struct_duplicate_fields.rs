#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct Destination {
    value: u32,
    #[lift(from = value)]
    alias: u32,
}

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct DuplicateAttribute {
    #[lift(from = value)]
    #[lift(from = other)]
    value: u32,
}

fn main() {}
