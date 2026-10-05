#[derive(Debug, errlanes::Rejection)]
enum Source {
    Shared,
}

#[errlanes::compose(prefix = "X")]
enum NotASourceList {}

#[errlanes::compose(Source as)]
enum MissingPrefix {}

#[errlanes::compose(Source<u32>)]
enum Generic {}

#[errlanes::compose(&'static Source)]
enum Reference {}
fn main() {}
