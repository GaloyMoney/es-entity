#[derive(errlanes::Lift)]
struct MissingRegistration { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source)]
struct MissingVariant { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value)]
#[lift(Other, variant = Value)]
struct MultipleSources { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value, variant = Other)]
struct DuplicateVariant { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value, strict, unhandled = fatal)]
struct ConflictingMode { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value, unhandled = ignored)]
struct InvalidMode { value: u32 }

#[derive(errlanes::Lift)]
#[lift(Source, variant = Value, with = mapper)]
struct UnsupportedMapper { value: u32 }

fn main() {}
