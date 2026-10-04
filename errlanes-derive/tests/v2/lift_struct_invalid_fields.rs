enum Source { Value { value: u32, extra: u32 } }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct Missing { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct Unknown {
    #[lift(from = absent)]
    value: u32,
    extra: u32,
}

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct WrongType { value: String, extra: u32 }

fn main() {}
