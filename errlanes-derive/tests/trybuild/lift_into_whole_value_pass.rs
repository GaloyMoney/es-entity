struct Wide(u64);

impl From<Source> for Wide {
    fn from(source: Source) -> Self {
        Self(u64::from(source.0))
    }
}

struct Source(u8);

#[derive(errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source, into)]
    Value(Wide),
}

#[derive(Debug, errlanes::Classify, errlanes::Lift)]
#[lift(Source)]
enum Wrapper {
    #[classify(delegate, from)]
    #[lift(Source, into)]
    Value(Inner),
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "INNER")]
struct Inner;

impl From<Source> for Inner {
    fn from(_: Source) -> Self {
        Self
    }
}

fn main() {}
