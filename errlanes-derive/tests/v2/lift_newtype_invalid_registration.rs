struct Source { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source)]
struct Missing(u32);

#[derive(errlanes::Lift)]
#[lift(Source, field = value, field = value)]
struct Duplicate(u32);

#[derive(errlanes::Lift)]
#[lift(Source, field = value)]
#[lift(Source, field = value)]
struct Multiple(u32);

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value, field = value)]
struct Variant(u32);

#[derive(errlanes::Lift)]
#[lift(Source, field = value, unhandled = fatal)]
struct Partial(u32);

#[derive(errlanes::Lift)]
#[lift(Source, field = value, into)]
struct Into(u32);

#[derive(errlanes::Lift)]
#[lift(Source, field = value, with = mapper)]
struct Mapper(u32);

#[derive(errlanes::Lift)]
#[lift(Source, field = value)]
struct FieldAttribute(#[lift(from = value)] u32);

fn main() {}
