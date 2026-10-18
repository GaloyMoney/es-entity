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
#[lift(Source, unhandled = fatal)]
enum Destination {
    #[lift(Source, into)]
    Whole(Wide),
}

fn main() {}
