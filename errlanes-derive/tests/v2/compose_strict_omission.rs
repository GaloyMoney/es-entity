#[derive(Debug, errlanes::Rejection)]
enum Source {
    One,
    Added,
}
#[errlanes::compose]
#[derive(Debug)]
#[lift(Source)]
enum Destination {
    #[lift(Source::One)]
    One,
}
fn main() {}
