#[derive(Debug, errlanes::Rejection)]
enum Source {
    One,
}

// A family listed twice, even under a prefix, would import every case twice.
#[errlanes::compose(Source, Source as Second)]
enum Twice {}

#[errlanes::compose(Source)]
#[lift(Source)]
enum AlreadyLifted {
    #[compose(merge)]
    #[rejection(code = "ONE")]
    One,
}
fn main() {}
