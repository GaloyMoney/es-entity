enum Source {
    A,
    B,
}

struct Wide;

impl From<Source> for Wide {
    fn from(_: Source) -> Self {
        Self
    }
}

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source, into)]
    Whole(Wide),
    #[lift(Source::A)]
    A,
}

fn main() {}
