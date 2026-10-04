#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct UnsupportedAttribute {
    #[lift(field = value)]
    value: u32,
}

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct ExtraOption {
    #[lift(from = value, with = mapper)]
    value: u32,
}

fn main() {}
