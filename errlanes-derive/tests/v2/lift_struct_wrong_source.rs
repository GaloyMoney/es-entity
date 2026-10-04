enum Source { Value(u32) }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct WrongShape { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Missing)]
struct WrongVariant { value: u32 }

fn main() {}
