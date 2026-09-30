#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Source {
    #[error("one")]
    One,
    #[error("two")]
    Two,
}
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
#[lift(Source, unhandled = fatal)]
enum Partial {
    #[lift(Source::One)]
    #[error("one")]
    One,
}
fn main() {
    let _: Partial = Source::Two.into();
}
