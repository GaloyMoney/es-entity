#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Source {
    #[error("one")]
    One,
    #[error("added case")]
    Added,
}
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
#[lift(Source)]
enum Destination {
    #[lift(Source::One)]
    #[error("one")]
    One,
}
fn main() {}
