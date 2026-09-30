#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Source {
    #[error("one")]
    One,
}

#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
#[lift(Source)]
enum Mixed {
    #[compose(flatten)]
    Whole(Source),
    #[lift(Source::One)]
    #[error("one")]
    One,
}

#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
enum Twice {
    #[compose(flatten)]
    First(Source),
    #[compose(flatten)]
    Second(Source),
}
fn main() {}
