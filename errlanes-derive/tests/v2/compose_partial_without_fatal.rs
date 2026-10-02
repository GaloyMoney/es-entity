#[derive(Debug, errlanes::Rejection)]
enum Source {
    One,
    Two,
}
#[errlanes::compose]
#[derive(Debug)]
#[lift(Source, unhandled = fatal)]
enum Destination {
    #[lift(Source::One)]
    One,
}
fn main() {
    let source: errlanes::Fail<Source, errlanes::lanes!()> = errlanes::Fail::Rejected(Source::Two);
    let _: errlanes::Fail<Destination, errlanes::lanes!()> = source.lift();
}
