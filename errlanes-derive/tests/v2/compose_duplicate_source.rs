#[derive(Debug, errlanes::Rejection)]
enum Source {
    One,
}

#[errlanes::compose]
#[derive(Debug)]
#[lift(Source)]
enum Mixed {
    #[compose(flatten)]
    Whole(Source),
    #[lift(Source::One)]
    One,
}

#[errlanes::compose]
#[derive(Debug)]
enum Twice {
    #[compose(flatten)]
    First(Source),
    #[compose(flatten)]
    Second(Source),
}
fn main() {}
