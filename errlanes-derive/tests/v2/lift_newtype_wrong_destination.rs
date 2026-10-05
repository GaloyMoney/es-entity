struct Source { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, field = value)]
struct Empty();

#[derive(errlanes::Lift)]
#[lift(Source, field = value)]
struct Multiple(u32, u32);

#[derive(errlanes::Lift)]
#[lift(Source, field = value)]
struct Named { value: u32 }

fn main() {}
