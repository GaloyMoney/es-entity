struct Source { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
struct WithVariant;

#[derive(errlanes::Lift)]
#[lift(Source, field = value)]
struct WithField;

#[derive(errlanes::Lift)]
struct Missing;

#[derive(errlanes::Lift)]
#[lift(Source)]
#[lift(Source)]
struct Multiple;

fn main() {}
