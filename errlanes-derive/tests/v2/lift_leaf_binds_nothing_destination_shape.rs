enum Source {
    Value(u8),
}

struct Gone;

#[derive(errlanes::Lift)]
#[lift(Source)]
enum TwoFields {
    #[lift(Source::Value { .. })]
    Value(Gone, Gone),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum NoFieldsNamed {
    #[lift(Source::Value { .. })]
    Value { first: Gone, second: Gone },
}

fn main() {}
