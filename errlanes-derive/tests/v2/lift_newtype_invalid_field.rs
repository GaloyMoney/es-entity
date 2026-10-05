struct Source { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, field = missing)]
struct Missing(u32);

#[derive(errlanes::Lift)]
#[lift(Source, field = value)]
struct WrongType(String);

mod hidden {
    pub struct Source { value: u32 }
}

#[derive(errlanes::Lift)]
#[lift(hidden::Source, field = value)]
struct Private(u32);

fn main() {}
