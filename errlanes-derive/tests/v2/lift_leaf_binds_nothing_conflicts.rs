enum Source {
    Value(u8),
}

struct Gone;

impl From<u8> for Gone {
    fn from(_: u8) -> Self {
        Gone
    }
}

fn mapper(_: Source) -> Destination {
    Destination::Plain
}

fn big(_: &u8) -> bool {
    true
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum WithInto {
    #[lift(Source::Value { .. }, into)]
    Value(Gone),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum WithField {
    #[lift(Source::Value { .. }, field = value)]
    Value(Gone),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum WithWhen {
    #[lift(Source::Value { .. }, when = big)]
    Value(Gone),
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum WithWith {
    #[lift(Source::Value { .. }, with = mapper)]
    Value(Gone),
}

enum Destination {
    Plain,
}

fn main() {}
