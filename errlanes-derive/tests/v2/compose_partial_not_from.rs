#[derive(Debug, errlanes::Rejection)]
enum Source {
    One,
    Two,
}
#[errlanes::compose]
#[derive(Debug)]
#[lift(Source, unhandled = fatal)]
enum Partial {
    #[lift(Source::One)]
    One,
}
fn main() {
    let _: Partial = Source::Two.into();
}
