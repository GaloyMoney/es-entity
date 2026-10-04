enum Source {
    Value(u8),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Duplicate {
    #[lift(Source::Value, into, into)]
    Value(u64),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum IntoWith {
    #[lift(Source::Value, into, with = mapper)]
    Value(u64),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum WithInto {
    #[lift(Source::Value, with = mapper, into)]
    Value(u64),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum IntoField {
    #[lift(Source::Value, into, field = value)]
    Value(u64),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum FieldInto {
    #[lift(Source::Value, field = value, into)]
    Value(u64),
}

fn main() {}
