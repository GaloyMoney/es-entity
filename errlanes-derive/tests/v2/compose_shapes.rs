#[errlanes::compose]
enum Unit {
    #[compose(flatten)]
    Child,
}
#[errlanes::compose]
enum Named {
    #[compose(flatten)]
    Child { source: Source },
}
#[errlanes::compose]
enum Multiple {
    #[compose(flatten)]
    Child(Source, Source),
}
#[errlanes::compose]
enum Reference {
    #[compose(flatten)]
    Child(&'static Source),
}
fn main() {}
