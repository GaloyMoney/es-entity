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
enum Destination {
    #[lift(Source::One)]
    #[error("one")]
    One,
}
fn main() {
    let source: errlanes::Fail<Source, errlanes::lanes!()> = errlanes::Fail::Rejected(Source::Two);
    let _: errlanes::Fail<Destination, errlanes::lanes!()> = source.lift();
}
